//! Host control limits bound authority records without evicting an accepted operation.
pub(super) const VERSION: u32 = 1;
pub(crate) const STORE_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const RECEIPT_LIMIT: usize = 2048;
pub(super) const SCHEDULE_LIMIT: usize = 100;
pub(crate) const OCCURRENCE_LIMIT: usize = 1000;
pub(super) const RETRY_HEADROOM_BYTES: u64 = 128 * 1024;
pub(super) const AREA: &str = "execution";
pub(super) const LOCK_ATTEMPTS: usize = 20;
pub(super) const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(5);
pub(crate) const MIN_INTERVAL: u64 = 60;
pub(crate) const MAX_INTERVAL: u64 = 30 * 24 * 60 * 60;
pub(crate) const MIN_DURATION: u64 = 60;
pub(crate) const MAX_DURATION: u64 = 24 * 60 * 60;
pub(crate) const DEFAULT_INTERVAL: u64 = 24 * 60 * 60;
pub(crate) const DEFAULT_DURATION: u64 = 30 * 60;
