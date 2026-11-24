//! Separate private dispatcher status survives store capacity exhaustion and is shared by daemon owners.
use super::{Repository, constants::*, repository::locked_named};
use serde::{Deserialize, Serialize};
const STATUS_BYTES: u64 = 16 * 1024;
const ERROR_CHARS: usize = 1024;
/// A revision lets another daemon's explicit Retry clear an owner's cached storage-write failure.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DispatcherStatus {
    version: u32,
    binding: String,
    pub revision: u64,
    pub error: Option<String>,
}
impl Repository {
    fn dispatcher_filename(&self) -> String {
        format!("{}.dispatcher.json", self.key)
    }
    fn read_dispatcher(&self) -> Result<Option<DispatcherStatus>, String> {
        let bytes =
            crate::permissions::read_local(AREA, &self.dispatcher_filename(), STATUS_BYTES)?;
        let Some(bytes) = bytes else { return Ok(None) };
        let status:DispatcherStatus=serde_json::from_slice(&bytes).map_err(|_|"Saved dispatcher status is unreadable. Restore it before retrying recurring execution.")?;
        if status.version != VERSION || status.binding != self.key {
            return Err(
                "Saved dispatcher status has an unsupported version or project identity.".into(),
            );
        }
        Ok(Some(status))
    }
    fn write_dispatcher(
        &self,
        previous: Option<DispatcherStatus>,
        error: Option<String>,
    ) -> Result<DispatcherStatus, String> {
        let revision = previous
            .map_or(Some(1), |p| p.revision.checked_add(1))
            .ok_or("Dispatcher status revision is exhausted.")?;
        let status = DispatcherStatus {
            version: VERSION,
            binding: self.key.clone(),
            revision,
            error: error.map(|s| s.chars().take(ERROR_CHARS).collect()),
        };
        let bytes = serde_json::to_vec(&status).map_err(|e| e.to_string())?;
        crate::permissions::replace_local(AREA, &self.dispatcher_filename(), &bytes)?;
        Ok(status)
    }
    /// Status is independent of execution-history capacity and uses one short cross-daemon lease.
    pub(crate) fn dispatcher_status(&self) -> Result<Option<DispatcherStatus>, String> {
        locked_named(&format!("dispatcher-{}.lock", self.key), || {
            self.read_dispatcher()
        })
    }
    /// Failures latch without rewriting accepted receipts or deleting history to make room.
    pub(crate) fn halt_dispatcher(&self, error: String) -> Result<DispatcherStatus, String> {
        locked_named(&format!("dispatcher-{}.lock", self.key), || {
            let before = self.read_dispatcher()?;
            self.write_dispatcher(before, Some(error))
        })
    }
    /// Recovery and clearing shared status are serialized against any daemon reporting a new failure.
    pub(crate) fn retry_dispatcher(
        &self,
        recover: impl FnOnce() -> Result<(), String>,
    ) -> Result<DispatcherStatus, String> {
        locked_named(&format!("dispatcher-{}.lock", self.key), || {
            let before = self.read_dispatcher()?;
            recover()?;
            self.write_dispatcher(before, None)
        })
    }
}
