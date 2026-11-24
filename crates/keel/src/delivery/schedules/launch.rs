//! Occurrences use exactly the same isolated task launcher and normal provider permissions.
use super::super::{
    Failure, Provenance, Service,
    launch::{LaunchPlan, base_sha},
};
use crate::{
    execution::{Occurrence, Phase, Schedule},
    tasks::Source,
};
use std::sync::Arc;

impl Service {
    pub(super) async fn occurrence_owner(
        self: &Arc<Self>,
        occurrence: &Occurrence,
    ) -> Result<Option<Source>, Failure> {
        let receipt = self.read_receipt(&occurrence.receipt).await?;
        let plan = receipt
            .result
            .and_then(|v| serde_json::from_value::<LaunchPlan>(v).ok());
        let Some(plan) = plan else { return Ok(None) };
        let service = self.clone();
        crate::serve::in_blocking(move || service.owner(&plan.task).map(Some)).await
    }

    pub(super) async fn launch_occurrence(
        self: &Arc<Self>,
        occurrence: &Occurrence,
    ) -> Result<(), Failure> {
        let service = self.clone();
        let id = occurrence.id.clone();
        let lease = crate::serve::in_blocking(move || {
            crate::permissions::try_lease(&format!(
                "occurrence-{}-{id}.lock",
                crate::permissions::key_of(&service.root)?
            ))
        })
        .await
        .map_err(Failure::Unavailable)?;
        let _lease = lease.ok_or_else(|| {
            Failure::Conflict("Occurrence preparation is already in progress.".into())
        })?;
        let service = self.clone();
        let id = occurrence.id.clone();
        let active = crate::serve::in_blocking(move || {
            service.execution.read().map(|store| {
                store.active.as_deref() == Some(&id)
                    && store
                        .occurrences
                        .get(&id)
                        .is_some_and(|o| o.phase == Phase::Reserved)
            })
        })
        .await
        .map_err(Failure::Unavailable)?;
        if !active {
            return Ok(());
        }
        let receipt = self.read_receipt(&occurrence.receipt).await?;
        if !matches!(receipt.phase, Phase::Reserved | Phase::Preparing) {
            return Ok(());
        }
        let service = self.clone();
        let id = occurrence.receipt.clone();
        let plan = crate::serve::in_blocking(move || service.prepare_occurrence(&id)).await?;
        if !plan.dispatched {
            self.dispatch(&occurrence.receipt, plan.clone()).await?;
        }
        let service = self.clone();
        let id = occurrence.id.clone();
        crate::serve::in_blocking(move || {
            service.execution.change(|store| {
                let occurrence = store
                    .occurrences
                    .get_mut(&id)
                    .ok_or("Occurrence unavailable.")?;
                let schedule: Schedule =
                    serde_json::from_value(store.receipts[&id].input["schedule"].clone())
                        .map_err(|e| e.to_string())?;
                occurrence.task = Some(plan.task);
                occurrence.phase = Phase::Running;
                occurrence.started_at = Some(service.clock.now());
                occurrence.deadline = Some(
                    service
                        .clock
                        .now()
                        .saturating_add(super::milliseconds(schedule.duration)),
                );
                Ok(())
            })
        })
        .await
        .map_err(Failure::Unavailable)?;
        super::changed();
        Ok(())
    }

    fn prepare_occurrence(&self, id: &str) -> Result<LaunchPlan, Failure> {
        let receipt = self.receipt(id)?;
        if let Some(plan) = receipt.result {
            return serde_json::from_value(plan).map_err(|_| {
                Failure::Unavailable("Saved recurring preparation is unreadable.".into())
            });
        }
        let schedule: Schedule = serde_json::from_value(receipt.input["schedule"].clone())
            .map_err(|_| {
                Failure::Unavailable("Saved recurring instruction is unreadable.".into())
            })?;
        if receipt.input["automatic"] == true {
            let current = self
                .execution
                .read()
                .map_err(Failure::Unavailable)?
                .schedules
                .get(&schedule.id)
                .cloned();
            if !current.is_some_and(|current| {
                current.enabled
                    && current.revision == schedule.revision
                    && crate::permissions::trust_stamp(&self.root).as_deref()
                        == Some(&schedule.trust)
            }) {
                return Err(Failure::Conflict(
                    "Recurring authorization changed before dispatch. No agent was started.".into(),
                ));
            }
        }
        if !self.runtime.available(&schedule.provider) {
            return Err(Failure::Invalid(
                "The scheduled provider is unavailable. Install it and run a new occurrence."
                    .into(),
            ));
        }
        let sha = base_sha(&self.root, &schedule.base).map_err(Failure::Invalid)?;
        let nonce = super::super::service::id()?;
        let wt = format!("task-{nonce}");
        let owner = Source {
            lane: nonce.clone(),
            session: nonce,
            provider: schedule.provider.clone(),
            checkout: crate::worktree::path_of(&self.root, &wt)
                .map_err(Failure::Invalid)?
                .to_string(),
            wt: Some(wt),
        };
        let task = crate::tasks::key_for(&owner);
        let plan = LaunchPlan {
            task: task.clone(),
            owner,
            base: schedule.base,
            sha,
            title: schedule.name.clone(),
            source: Provenance::Recurring {
                definition: schedule.id,
                revision: schedule.revision,
                name: schedule.name,
                instruction: schedule.prompt.clone(),
            },
            prompt: format!(
                "Carry out this saved recurring instruction in this isolated checkout. Follow project instructions, normal permissions and the relevant checks. Publication remains a person's action.\n\n<recurring-instruction>\n{}\n</recurring-instruction>",
                schedule.prompt
            ),
            dispatched: false,
            preview: None,
            warnings: vec![],
        };
        self.execution
            .change(|store| {
                let receipt = store
                    .receipts
                    .get_mut(id)
                    .ok_or("Occurrence receipt unavailable.")?;
                receipt.task = Some(task);
                receipt.result = Some(serde_json::to_value(&plan).map_err(|e| e.to_string())?);
                receipt.phase = Phase::Preparing;
                Ok(())
            })
            .map_err(Failure::Unavailable)?;
        Ok(plan)
    }
}
