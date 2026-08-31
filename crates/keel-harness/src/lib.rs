//! Supervising the user's installed `claude` binary.
//!
//! Keel does not embed an agent SDK. It drives the `claude` CLI the developer already has installed
//! and already logged in, which means their subscription covers their own usage and Keel never
//! handles an API key.
//!
//! That choice has one sharp consequence, and it shapes this whole crate: `--bare` cannot be used,
//! because bare mode never reads OAuth credentials. Without `--bare`, a `-p` session loads and
//! executes the *repository's* own `.claude/settings.json` hooks and `.mcp.json` servers with no
//! trust prompt. See [`crate::trust`] — quarantine is not optional.

pub mod trust;

pub use trust::{QuarantineReport, quarantine};
