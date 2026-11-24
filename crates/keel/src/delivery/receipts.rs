//! Durable status reconciliation never redispatches uncertain runtime acceptance.
use super::{Failure, Service, launch::LaunchPlan};
use crate::execution::{Phase, Receipt};
use std::sync::Arc;
impl Service {
    /// Receipt status can confirm acceptance after a lost response, but never redispatches uncertain work.
    pub(crate) async fn launch_status(self: &Arc<Self>, id: &str) -> Result<Receipt, Failure> {
        let receipt = self.read_receipt(id).await?;
        if let Some(value) = &receipt.result
            && let Ok(plan) = serde_json::from_value::<LaunchPlan>(value.clone())
            && plan.dispatched
        {
            let status = self
                .runtime
                .receipt(plan.owner.lane.clone(), id.into())
                .await;
            match status {
                Ok(status) if status.accepted => {let (phase,error)=observed_phase(status);self.save_phase(id,phase,error).await?;},
                _ => self.save_phase(id,Phase::Unconfirmed,Some("Task creation is unconfirmed. No new agent will start from this request.".into())).await?,
            }
        }
        self.read_receipt(id).await
    }

    pub(crate) fn receipt(&self, id: &str) -> Result<Receipt, Failure> {
        self.execution.read().map_err(Failure::Unavailable)?.receipts.get(id).cloned().ok_or_else(|| Failure::Missing("Saved request is unavailable. Do not start another copy until its state is known.".into()))
    }

    pub(super) async fn read_receipt(self: &Arc<Self>, id: &str) -> Result<Receipt, Failure> {
        let owner = self.clone();
        let id = id.to_string();
        crate::serve::in_blocking(move || owner.receipt(&id)).await
    }

    pub(super) async fn save_phase(
        self: &Arc<Self>,
        id: &str,
        phase: Phase,
        error: Option<String>,
    ) -> Result<(), Failure> {
        let owner = self.clone();
        let id = id.to_string();
        crate::serve::in_blocking(move || owner.phase(&id, phase, error)).await
    }

    pub(super) fn phase(
        &self,
        id: &str,
        phase: Phase,
        error: Option<String>,
    ) -> Result<(), Failure> {
        if self
            .receipt(id)
            .is_ok_and(|receipt| receipt.phase == phase && receipt.error == error)
        {
            return Ok(());
        }
        self.execution
            .change(|store| {
                let receipt = store
                    .receipts
                    .get_mut(id)
                    .ok_or("Saved request unavailable.")?;
                if matches!(
                    receipt.phase,
                    Phase::Completed | Phase::Failed | Phase::Stopped | Phase::Interrupted
                ) && matches!(
                    phase,
                    Phase::Reserved
                        | Phase::Preparing
                        | Phase::Queued
                        | Phase::Accepted
                        | Phase::Unconfirmed
                ) {
                    return Ok(());
                }
                receipt.phase = phase;
                receipt.error = error;
                receipt.updated_at = self.clock.now();
                Ok(())
            })
            .map_err(Failure::Unavailable)
    }
}

/// Completion records a stopped stream separately from successful provider execution.
pub(super) fn observed_phase(status: crate::chat::ReceiptStatus) -> (Phase, Option<String>) {
    let phase = if status.interrupted {
        Phase::Interrupted
    } else if status.error.is_some() {
        Phase::Failed
    } else if status.finished {
        Phase::Completed
    } else {
        Phase::Accepted
    };
    (phase, status.error)
}
