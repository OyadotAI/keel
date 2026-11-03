//! Parent-authorized child activity is shared by terminal indicators and read-only progress views.
mod agy;
mod claude;
mod claude_result;
mod codex;
mod discovery;
mod json;
mod model;
mod observer;
mod output;

pub use model::{Availability, Detail, Invocation, OutputLine, Snapshot, Status};
pub use observer::{Observer, Roots};

/// Grid output requests share one bounded parent-authorized observation batch.
pub const OUTPUT_BATCH_LIMIT: usize = 8;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod limits_tests;
