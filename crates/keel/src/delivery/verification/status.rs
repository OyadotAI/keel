//! Operation status reconciles private check proof and never starts a replacement.
use super::*;
impl Service {
    /// Completion can be recovered from its private observation after a controller or HTTP connection ends.
    pub(crate) async fn check_status(self: &Arc<Self>, id: &str) -> Result<Receipt, Failure> {
        let receipt = self.read_receipt(id).await?;
        let Some(value) = receipt.result.clone() else {
            return Ok(receipt);
        };
        let mut plan: CheckPlan = serde_json::from_value(value)
            .map_err(|_| Failure::Unavailable("Saved check request is unreadable.".into()))?;
        if matches!(
            receipt.phase,
            Phase::Completed | Phase::Stopped | Phase::FailedBeforeAcceptance
        ) {
            return Ok(receipt);
        }
        let owner = self.clone();
        let proof = format!("evidence-{:x}", Sha256::digest(id.as_bytes()));
        let observed = crate::serve::in_blocking(move || {
            owner
                .execution
                .read()
                .map(|store| store.receipts.get(&proof).cloned())
        })
        .await
        .map_err(Failure::Unavailable)?;
        let active = self.active.locked().contains(id);
        if let Some((check, _)) = observed
            .filter(|proof| proof.task.as_deref() == Some(&plan.task))
            .and_then(|proof| super::super::evidence::observation(&proof))
            .filter(|(check, owner)| check.id == id && owner == &plan.owner)
        {
            let phase = if check.status == "aborted" {
                Phase::Stopped
            } else {
                Phase::Completed
            };
            plan.check = Some(check);
            self.finish_check_receipt(id, plan, phase, None).await?;
        } else if !active
            && (plan.generation != self.generation
                || self.clock.now().saturating_sub(receipt.updated_at) >= STATUS_GRACE_MS)
        {
            self.save_phase(id,Phase::Unconfirmed,Some("Check completion is unconfirmed. Inspect its saved result before starting another check.".into())).await?;
        }
        self.read_receipt(id).await
    }

    /// Read-only reconciliation chooses the saved operation kind and never dispatches new work.
    pub(crate) async fn operation_status(self: &Arc<Self>, id: &str) -> Result<Receipt, Failure> {
        let receipt = self.read_receipt(id).await?;
        match receipt.kind.as_str() {
            "check" => self.check_status(id).await,
            "batch" => self.batch_status(id).await,
            _ => self.launch_status(id).await,
        }
    }
}
