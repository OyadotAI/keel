//! Storage and payload limits bound persisted delivery history without silently dropping work.
pub(super) const VERSION: u32 = 1;
pub(super) const STORE_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const TASK_LIMIT: usize = 256;
pub(super) const CORRECTION_LIMIT: usize = 500;
pub(super) const PREVIEW_LIMIT: usize = 100;
pub(super) const TEXT_LIMIT: usize = 20 * 1024;
pub(super) const PAGE_LIMIT: usize = 100;
pub(super) const TITLE_CHARS: usize = 160;
pub(super) const BINDING_MARKER: &[u8] = b"keel-delivery-v1\n";
pub(super) const STATUS_GRACE_MS: u64 = 10_000;
pub(super) const MILLISECONDS_PER_SECOND: u64 = 1000;
pub(super) const RUNNER_TICK: std::time::Duration = std::time::Duration::from_secs(2);

pub(super) const NONCE_BYTES: usize = 16;
pub(super) const TICK_GAP_MS: u64 = 30_000;
