use crate::{Check, Dimension, Finding, Fix, RepoContext, Severity};

/// Whether the repository explains itself to an agent.
///
/// An agent working without project instructions rediscovers conventions by guessing, and guesses
/// inconsistently between sessions. This is the cheapest quality lever in the whole report.
pub struct AgentInstructions;

const INSTRUCTION_FILES: &[&str] = &["CLAUDE.md", "AGENTS.md", ".claude/CLAUDE.md"];

impl Check for AgentInstructions {
    fn id(&self) -> &'static str {
        "agent/no-instructions"
    }

    fn dimension(&self) -> Dimension {
        Dimension::AgentLegibility
    }

    fn run(&self, ctx: &RepoContext) -> Vec<Finding> {
        if ctx.has_any(INSTRUCTION_FILES.iter().copied()) {
            return Vec::new();
        }

        vec![Finding::new(
            self.id(),
            self.dimension(),
            Severity::Medium,
            "No agent instructions (CLAUDE.md or AGENTS.md)",
            "Without project instructions an agent infers your conventions from whatever file it \
             happens to read first, and infers them differently next session.",
            Fix::Automatic {
                description: "Generate a CLAUDE.md from what the scan already learned: package \
                              manager, test and typecheck commands, directory layout, and the \
                              deploy path."
                    .to_string(),
            },
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    #[test]
    fn accepts_either_filename() {
        for name in ["CLAUDE.md", "AGENTS.md"] {
            let (_dir, ctx) = fixture(&[(name, "# guidance")]);
            assert!(
                AgentInstructions.run(&ctx).is_empty(),
                "{name} not accepted"
            );
        }
    }

    #[test]
    fn flags_a_repo_with_neither() {
        let (_dir, ctx) = fixture(&[("README.md", "hi")]);
        assert_eq!(AgentInstructions.run(&ctx).len(), 1);
    }
}
