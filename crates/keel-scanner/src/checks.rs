//! The default check set.
//!
//! Each check is small, independent and pure. Adding one is a matter of implementing [`Check`] and
//! appending it to [`default_checks`] — the ordering there does not matter, since [`crate::scan`]
//! sorts findings by severity.

mod agent_instructions;
mod ci;
mod env_hygiene;
mod hosting;
mod pipeline;
mod practices;
mod production_shape;
mod secrets;
mod tests_present;
mod untrusted_agent_config;
mod workers_compat;
mod wrangler;

use crate::Check;

/// Every check Keel runs by default.
pub fn default_checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(untrusted_agent_config::UntrustedAgentConfig),
        Box::new(env_hygiene::SharedBindings),
        Box::new(workers_compat::WorkersCompat),
        Box::new(secrets::CommittedSecrets),
        Box::new(tests_present::TestsPresent),
        Box::new(ci::ContinuousIntegration),
        Box::new(agent_instructions::AgentInstructions),
        Box::new(production_shape::ProductionShape),
        Box::new(practices::ProductionPractices),
        Box::new(hosting::HostingFit),
        Box::new(pipeline::PipelineQuality),
    ]
}
