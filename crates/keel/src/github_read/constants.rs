//! Fixed GitHub origins and named traversal budgets prevent unbounded or retargeted reads.
pub(super) const API_VERSION: &str = "2026-03-10";
pub(super) const RESPONSE_BYTES: usize = 1024 * 1024;
pub(super) const BODY_BYTES: usize = 20 * 1024;
pub(super) const RECORD_LIMIT: usize = 500;
pub(super) const REST_PAGE: usize = 100;
pub(super) const THREAD_PAGE: usize = 50;
pub(super) const COMMENT_PAGE: usize = 20;
pub(super) const REQUEST_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
pub(super) const OPERATION_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
pub(super) const ERROR_CHARS: usize = 2000;
