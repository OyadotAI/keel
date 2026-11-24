//! A recoverable preparation state machine registers one exact checkout before shared chat acceptance.
use super::{Service, error::Failure, model::*, service::StartIssue};
use crate::{
    execution::{Phase, Receipt},
    tasks::Source,
};
use serde::{Deserialize, Serialize};

/// Prepared identities survive a crash between worktree creation, task registration and provider acceptance.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LaunchPlan {
    pub task: String,
    pub owner: Source,
    pub base: String,
    pub sha: String,
    pub title: String,
    pub source: Provenance,
    pub prompt: String,
    pub dispatched: bool,
    pub preview: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl Service {
    /// Duplicate HTTP requests share an operation lease and the same durable launch receipt.
    pub(crate) async fn start_issue(
        self: Arc<Self>,
        input: StartIssue,
    ) -> Result<Receipt, Failure> {
        if !crate::execution::valid_id(&input.receipt)
            || !crate::agent::PROVIDERS.contains(&input.provider.as_str())
        {
            return Err(Failure::Invalid(
                "Choose an installed coding agent and a valid request ID.".into(),
            ));
        }
        let owner = self.clone();
        let frozen = input.clone();
        let frozen = serde_json::to_value(&frozen).map_err(|e| Failure::Invalid(e.to_string()))?;
        let receipt_id = input.receipt.clone();
        let receipt = crate::serve::in_blocking(move || {
            owner
                .execution
                .reserve(&receipt_id, "issue", frozen, owner.clock.now())
        })
        .await
        .map_err(Failure::from)?;
        if !matches!(
            receipt.phase,
            Phase::Reserved | Phase::Preparing | Phase::FailedBeforeAcceptance
        ) {
            return self.launch_status(&input.receipt).await;
        }
        let operation = input.receipt.clone();
        let project = self.root.clone();
        let lease = crate::serve::in_blocking(move || {
            let binding = crate::permissions::key_of(&project)?;
            crate::permissions::try_lease(&format!("delivery-op-{binding}-{operation}.lock"))
        })
        .await
        .map_err(Failure::Unavailable)?;
        let Some(_lease) = lease else {
            return Ok(receipt);
        };
        self.save_phase(&input.receipt, Phase::Preparing, None)
            .await?;
        let result = self.prepare_issue(&input).await;
        match result {
            Ok(plan) => self.dispatch(&input.receipt, plan).await,
            Err(error) => {
                self.save_phase(
                    &input.receipt,
                    Phase::FailedBeforeAcceptance,
                    Some(error.message().into()),
                )
                .await?;
                self.read_receipt(&input.receipt).await
            }
        }
    }

    pub(super) async fn dispatch(
        self: &Arc<Self>,
        receipt_id: &str,
        mut plan: LaunchPlan,
    ) -> Result<Receipt, Failure> {
        if plan.dispatched {
            return self.launch_status(receipt_id).await;
        }
        let root = self.root.clone();
        let _preparation =
            crate::serve::in_blocking(move || crate::checkout_activity::hold(&root, false))
                .await
                .map_err(Failure::Conflict)?;
        let owner = self.clone();
        let setup = plan.clone();
        match crate::serve::in_blocking(move || owner.prepare_checkout(&setup)).await {
            Ok(warnings) => plan.warnings = warnings,
            Err(error) => {
                self.save_phase(
                    receipt_id,
                    Phase::FailedBeforeAcceptance,
                    Some(error.message().into()),
                )
                .await?;
                return self.read_receipt(receipt_id).await;
            }
        }
        let owner = self.clone();
        let id = receipt_id.to_string();
        let _acceptance =
            match crate::serve::in_blocking(move || owner.authorize_dispatch(&id)).await {
                Ok(lease) => lease,
                Err(error) => {
                    self.save_phase(
                        receipt_id,
                        Phase::FailedBeforeAcceptance,
                        Some(error.message().into()),
                    )
                    .await?;
                    return self.read_receipt(receipt_id).await;
                }
            };
        plan.dispatched = true;
        let owner = self.clone();
        let saved = plan.clone();
        let receipt = receipt_id.to_string();
        crate::serve::in_blocking(move || {
            owner.execution.change(|store| {
                let receipt = store
                    .receipts
                    .get_mut(&receipt)
                    .ok_or("Launch receipt unavailable.")?;
                receipt.result = Some(serde_json::to_value(&saved).map_err(|e| e.to_string())?);
                receipt.updated_at = owner.clock.now();
                Ok(())
            })
        })
        .await
        .map_err(Failure::Unavailable)?;
        let result = self
            .runtime
            .submit(
                receipt_id.into(),
                query(&plan.owner, plan.prompt.clone(), false),
            )
            .await;
        match result {
            Ok(_) => self.save_phase(receipt_id, Phase::Accepted, None).await?,
            Err(error) => {
                let status = self
                    .runtime
                    .receipt(plan.owner.lane.clone(), receipt_id.into())
                    .await;
                let phase = if status.is_ok_and(|s| s.accepted) {
                    Phase::Accepted
                } else {
                    Phase::Unconfirmed
                };
                self.save_phase(receipt_id, phase, Some(error)).await?;
            }
        }
        crate::events::emit(
            "delivery.changed",
            plan.owner.wt.as_deref(),
            serde_json::json!({"task":plan.task}),
        );
        self.read_receipt(receipt_id).await
    }
}
use std::sync::Arc;

/// Only a validated local branch can supply the frozen start commit; the root checkout never moves.
pub(crate) fn base_sha(root: &camino::Utf8Path, base: &str) -> Result<String, String> {
    if base.is_empty() || base.starts_with('-') {
        return Err("Choose a local base branch.".into());
    }
    let reference = format!("refs/heads/{base}");
    crate::git::run(root, &["check-ref-format", &reference])
        .map_err(|_| "Choose a valid local base branch.".to_string())?;
    crate::git::trimmed(
        root,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )
    .map_err(|_| "The selected local base branch is unavailable.".into())
}

/// Every delivery request takes the same permission mode and provider path as an ordinary chat send.
pub(super) fn query(owner: &Source, prompt: String, resume: bool) -> crate::agent::ChatQuery {
    crate::agent::ChatQuery {
        prompt,
        attachments: vec![],
        references: vec![],
        ops: Some("1".into()),
        session: resume.then(|| owner.session.clone()),
        mode: Some("acceptEdits".into()),
        wt: owner.wt.clone(),
        cwd: None,
        system: None,
        lane: Some(owner.lane.clone()),
        model: None,
        effort: None,
        provider: Some(owner.provider.clone()),
        design: None,
        auto_commit: Some(true),
    }
}
