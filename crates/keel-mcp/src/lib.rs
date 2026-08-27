//! Keel's tool surface, served to the agent over loopback MCP.
//!
//! This is the only way the agent can act. Built-in `Bash`, `Edit` and `Write` are denied by the
//! invocation's `--permission-mode dontAsk`, so every effect the agent has on the world passes
//! through a tool defined here — which is what makes the audit log complete and the approval gates
//! enforceable rather than advisory.

mod tools;

pub use tools::{Tool, ToolGroup, catalog};
