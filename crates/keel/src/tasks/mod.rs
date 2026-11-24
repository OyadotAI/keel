//! Shared task memory and explicit provider continuation live under the canonical project's .keel.
mod api;
mod handoff;
mod model;
mod ownership;
mod progress;
mod recovery;
#[cfg(test)]
mod recovery_tests;
mod repository;
mod stopping;
#[cfg(test)]
mod tests;
mod text;
mod tracking;
pub(crate) use api::{continue_task, inspect, save, stop};
pub(crate) use model::{Source, Task};
pub(crate) use ownership::hold as hold_ownership;
pub(crate) use progress::{TOOL, call, schema};
pub(crate) use recovery::{for_lane, for_session, get, key_for, list, lookup, owner, pin, recover};
pub(crate) use tracking::{destructive_guard, guard, input, observed, register, session};
