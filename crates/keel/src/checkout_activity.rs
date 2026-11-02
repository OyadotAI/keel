//! Shared activity locks let ordinary readers coexist while branch switches reserve the whole tree.
use camino::Utf8Path;
use keel_workspace::Real;
use sha2::{Digest, Sha256};

/// Closing the guard releases a cross-daemon reservation even during cancellation or panic.
pub struct Activity(std::fs::File);
impl Drop for Activity {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Agents share this lock; checkout-changing operations require it exclusively for their lifetime.
pub fn hold(root: &Utf8Path, exclusive: bool) -> Result<Activity, String> {
    let real = root.real().map_err(|e| e.to_string())?;
    let name = format!(
        "activity-{:x}.lock",
        Sha256::digest(real.as_str().as_bytes())
    );
    let file = crate::permissions::private_lock(&name)?;
    let result = if exclusive {
        file.try_lock()
    } else {
        file.try_lock_shared()
    };
    result.map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => "An agent or Git operation is using this checkout. Wait for it to finish or stop its turn before switching.".to_string(),
        std::fs::TryLockError::Error(e) => format!("Could not reserve this checkout: {e}"),
    })?;
    Ok(Activity(file))
}
