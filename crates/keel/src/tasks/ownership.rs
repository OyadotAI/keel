//! Terminal-lifetime handoff reservations do not block ordinary Git operations on an idle terminal.
use camino::Utf8Path;
use keel_workspace::Real;
use sha2::{Digest, Sha256};

pub(crate) struct Reservation(std::fs::File);
impl Drop for Reservation {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
/// Shared terminal leases exclude the brief ownership transfer across all local daemons.
pub(crate) fn hold(checkout: &Utf8Path, exclusive: bool) -> Result<Reservation, String> {
    let real = checkout.real().map_err(|e| e.to_string())?;
    let name = format!(
        "task-owner-{:x}.lock",
        Sha256::digest(real.as_str().as_bytes())
    );
    let file = crate::permissions::private_lock(&name)?;
    let result = if exclusive {
        file.try_lock()
    } else {
        file.try_lock_shared()
    };
    result.map_err(|_| {
        "The source agent still owns this checkout. Stop source work and retry.".to_string()
    })?;
    Ok(Reservation(file))
}
