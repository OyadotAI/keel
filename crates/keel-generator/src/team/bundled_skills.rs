//! Specialist skill bundles shipped with the default team, including their attribution.

macro_rules! bundled {
    ($($path:literal),+ $(,)?) => {
        &[$((concat!(".claude/skills/", $path), include_str!(concat!("skills/", $path)))),+]
    };
}

pub(super) const FILES: &[(&str, &str)] = bundled![
    "codebase-design/LICENSE",
    "codebase-design/SKILL.md",
    "codebase-design/SOURCE.md",
    "codebase-design/references/alternatives.md",
    "codebase-design/references/deepening.md",
    "code-review/LICENSE",
    "code-review/SKILL.md",
    "code-review/SOURCE.md",
    "code-review/references/design-signals.md",
    "diagnosing-bugs/LICENSE",
    "diagnosing-bugs/SKILL.md",
    "diagnosing-bugs/SOURCE.md",
    "diagnosing-bugs/assets/investigation.md",
    "research/LICENSE",
    "research/SKILL.md",
    "research/SOURCE.md",
    "research/assets/research-note.md",
    "handoff/LICENSE",
    "handoff/SKILL.md",
    "handoff/SOURCE.md",
    "handoff/assets/handoff.md",
    "writing-for-agents/LICENSE",
    "writing-for-agents/SKILL.md",
    "writing-for-agents/SOURCE.md",
];
