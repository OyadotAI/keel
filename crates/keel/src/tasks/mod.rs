//! Shared task memory and explicit provider continuation live under the canonical project's .keel.
mod api;
mod handoff;
mod model;
mod ownership;
mod progress;
mod repository;
mod stopping;
#[cfg(test)]
mod tests;
mod text;
mod tracking;
pub(crate) use api::{continue_task, inspect, save, stop};
pub(crate) use model::Source;
pub(crate) use ownership::hold as hold_ownership;
pub(crate) use progress::{TOOL, call, schema};
pub(crate) use tracking::{destructive_guard, guard, input, observed, register, session};
