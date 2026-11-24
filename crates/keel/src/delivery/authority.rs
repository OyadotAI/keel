//! Final automatic authorization shares a lease with pause, edits and trust revocation.
use super::{Failure, Service};
use crate::{execution::Schedule, permissions::Lease};

impl Service {
    /// Preparation grants no execution authority; this fence stays held through shared chat acceptance.
    pub(super) fn authorize_dispatch(&self, id: &str) -> Result<Option<Lease>, Failure> {
        let receipt = self.receipt(id)?;
        if receipt.kind != "scheduled" || receipt.input["automatic"] != true {
            return Ok(None);
        }
        let lease = crate::permissions::acceptance_lease(&self.root).map_err(Failure::Conflict)?;
        let captured: Schedule = serde_json::from_value(receipt.input["schedule"].clone())
            .map_err(|_| {
                Failure::Unavailable("Saved recurring authorization is unreadable.".into())
            })?;
        let control = self.execution.read().map_err(Failure::Unavailable)?;
        let valid = control.schedules.get(&captured.id).is_some_and(|current| {
            current.enabled
                && current.revision == captured.revision
                && current.trust == captured.trust
                && crate::permissions::trust_stamp(&self.root).as_deref() == Some(&captured.trust)
        });
        if !valid {
            return Err(Failure::Conflict("Recurring execution was paused, edited or revoked during preparation. No agent was started.".into()));
        }
        Ok(Some(lease))
    }
}
