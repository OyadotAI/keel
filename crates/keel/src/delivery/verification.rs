//! Manual task checks have durable receipts and exact cancellation ownership independent of UI lifetime.
mod status;
use super::{CheckEvidence, Failure, Service, constants::STATUS_GRACE_MS, runtime::VerifyError};
use crate::{
    execution::{Phase, Receipt},
    lock::Locked,
    tasks::Source,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// The person captures the current owner; a later provider transfer cannot retarget this request.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckInput {
    pub receipt: String,
    pub owner: Source,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckPlan {
    task: String,
    owner: Source,
    generation: String,
    check: Option<CheckEvidence>,
}

impl Service {
    /// Return the accepted operation promptly; the daemon owns execution and persists the result afterward.
    pub(crate) async fn run_checks(
        self: Arc<Self>,
        task: String,
        input: CheckInput,
    ) -> Result<Receipt, Failure> {
        let owner = self.clone();
        let request = input.clone();
        let frozen_task = task.clone();
        let receipt = crate::serve::in_blocking(move || {
            owner.execution.reserve(
                &request.receipt,
                "check",
                serde_json::json!({"task":frozen_task,"request":request}),
                owner.clock.now(),
            )
        })
        .await
        .map_err(Failure::from)?;
        if !matches!(
            receipt.phase,
            Phase::Reserved | Phase::FailedBeforeAcceptance
        ) {
            return self.check_status(&input.receipt).await;
        }
        let owner = self.clone();
        let expected = input.owner.clone();
        let target = task.clone();
        let prepared = crate::serve::in_blocking(move || {
            if owner.owner(&target)? != expected {
                return Err(Failure::Conflict(
                    "Task ownership changed. Refresh before running checks.".into(),
                ));
            }
            if owner.runtime.pending(&expected) {
                return Err(Failure::Conflict(
                    "Answer the current request before running checks.".into(),
                ));
            }
            if !owner
                .records
                .read()
                .map_err(Failure::Unavailable)?
                .records
                .contains_key(&target)
            {
                owner.change(&target, 0, |_| Ok(()))?;
            }
            Ok(())
        })
        .await;
        if let Err(error) = prepared {
            self.save_phase(
                &input.receipt,
                Phase::FailedBeforeAcceptance,
                Some(error.message().into()),
            )
            .await?;
            return self.read_receipt(&input.receipt).await;
        }
        if self
            .runtime
            .busy(&input.owner)
            .await
            .map_err(Failure::Unavailable)?
        {
            self.save_phase(
                &input.receipt,
                Phase::FailedBeforeAcceptance,
                Some("Wait for this task's current turn and checks to finish.".into()),
            )
            .await?;
            return self.read_receipt(&input.receipt).await;
        }
        let plan = CheckPlan {
            task: task.clone(),
            owner: input.owner,
            generation: self.generation.clone(),
            check: None,
        };
        let owner = self.clone();
        let id = input.receipt.clone();
        let saved = plan.clone();
        let receipt = crate::serve::in_blocking(move || {
            owner.execution.change(|store| {
                let receipt = store
                    .receipts
                    .get_mut(&id)
                    .ok_or("Check receipt unavailable.")?;
                if !matches!(
                    receipt.phase,
                    Phase::Reserved | Phase::FailedBeforeAcceptance
                ) {
                    return Ok(None);
                }
                receipt.task = Some(saved.task.clone());
                receipt.result = Some(serde_json::to_value(&saved).map_err(|e| e.to_string())?);
                receipt.phase = Phase::Running;
                receipt.error = None;
                receipt.updated_at = owner.clock.now();
                Ok(Some(receipt.clone()))
            })
        })
        .await
        .map_err(Failure::Unavailable)?;
        let Some(receipt) = receipt else {
            return self.check_status(&input.receipt).await;
        };
        let active = self.track(&input.receipt);
        let service = self.clone();
        let id = input.receipt.clone();
        tokio::spawn(async move {
            let _active = active;
            if let Err(error) = service.execute_check(&id, plan).await {
                tracing::warn!("Check receipt could not be updated: {}", error.message());
            }
        });
        crate::events::emit("delivery.changed", None, serde_json::json!({"task":task}));
        Ok(receipt)
    }

    async fn execute_check(self: &Arc<Self>, id: &str, mut plan: CheckPlan) -> Result<(), Failure> {
        match self.runtime.verify(plan.owner.clone(), id.into()).await {
            Ok(check) => {
                let phase = if check.status == "aborted" {
                    Phase::Stopped
                } else {
                    Phase::Completed
                };
                plan.check = Some(check);
                self.finish_check_receipt(id, plan.clone(), phase, None)
                    .await?;
            }
            Err(VerifyError::Before(error)) => {
                self.finish_check_receipt(
                    id,
                    plan.clone(),
                    Phase::FailedBeforeAcceptance,
                    Some(error),
                )
                .await?
            }
            Err(VerifyError::Unknown(error)) => {
                self.finish_check_receipt(id, plan.clone(), Phase::Unconfirmed, Some(error))
                    .await?
            }
        }
        crate::events::emit(
            "delivery.changed",
            plan.owner.wt.as_deref(),
            serde_json::json!({"task":plan.task}),
        );
        Ok(())
    }

    async fn finish_check_receipt(
        self: &Arc<Self>,
        id: &str,
        plan: CheckPlan,
        phase: Phase,
        error: Option<String>,
    ) -> Result<(), Failure> {
        let service = self.clone();
        let id = id.to_string();
        crate::serve::in_blocking(move || {
            service.execution.change(|store| {
                let receipt = store
                    .receipts
                    .get_mut(&id)
                    .ok_or("Check receipt unavailable.")?;
                receipt.result = Some(serde_json::to_value(plan).map_err(|e| e.to_string())?);
                receipt.phase = phase;
                receipt.error = error;
                receipt.updated_at = service.clock.now();
                Ok(())
            })
        })
        .await
        .map_err(Failure::Unavailable)
    }

    /// Stop addresses the exact claimed check token, so a newer task turn is never its target.
    pub(crate) async fn stop_checks(
        self: &Arc<Self>,
        task: &str,
        id: &str,
    ) -> Result<bool, Failure> {
        let receipt = self.read_receipt(id).await?;
        if receipt.kind != "check" || receipt.task.as_deref() != Some(task) {
            return Err(Failure::Invalid(
                "This check belongs to another task.".into(),
            ));
        }
        let stopped = self
            .runtime
            .stop_check(id.into())
            .await
            .map_err(Failure::Unavailable)?;
        if stopped {
            self.save_phase(
                id,
                Phase::Stopping,
                Some("Stopping the requested check…".into()),
            )
            .await?;
        }
        Ok(stopped)
    }
}
