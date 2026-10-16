//! The agent team every project gets, and the lifecycle that runs them.
//!
//! Three reviewers read a change after it is written (`reviewer`, `security`, `reliability`).
//! Four more stand around it: `pm` decides whether and how small, `designer` owns what a person
//! sees, `principal` designs it before it is written, `qa` runs the real thing afterwards. The
//! agents alone are not the point — an agent nobody calls is a file — so [`LIFECYCLE`] goes into
//! `CLAUDE.md` and says in what order the main agent calls them and what it hands each one.
//!
//! These are Keel's own pipeline agents with Keel taken out: they name no module, port or
//! primitive of this repository, and each one starts by reading the project's `CLAUDE.md`, which
//! is where a project's own bar belongs. Scaffolds and `/api/adopt` both write from [`agents`], so
//! a new project and an adopted one get the same team.
//!
//! [`RULES`] is the other half: what "good code" means here, so `principal` has something to hold
//! a change to other than taste. It is the house style of Oya's browser repository (agentchrome)
//! with the TypeScript taken out — small units, named constants, facades, command maps,
//! repositories, a composition root, tests named after rules — offered as defaults a project
//! edits, never as a second authority over its own linter.

#[path = "team/bundled_skills.rs"]
mod bundled_skills;

/// The heading [`LIFECYCLE`] opens with. The scanner looks for it (case-insensitively) to decide
/// whether a `CLAUDE.md` already says how work flows, so it is not reworded lightly.
pub const LIFECYCLE_HEADING: &str = "## How a request becomes a change";

/// Every agent in the team, as `(repository path, file body)`.
pub fn agents() -> Vec<(&'static str, &'static str)> {
    vec![
        (".claude/agents/pm.md", PM_AGENT),
        (".claude/agents/designer.md", DESIGNER_AGENT),
        (".claude/agents/principal.md", PRINCIPAL_AGENT),
        (".claude/agents/qa.md", QA_AGENT),
        (".claude/agents/oya-browser.md", OYA_BROWSER_AGENT),
        (
            ".claude/agents/reviewer.md",
            crate::cloudflare::REVIEWER_AGENT,
        ),
        (".claude/agents/security.md", crate::stack::SECURITY_AGENT),
        (
            ".claude/agents/reliability.md",
            crate::stack::RELIABILITY_AGENT,
        ),
    ]
}

/// Agents and the skills, reference material, and output assets they depend on.
pub fn files() -> Vec<(&'static str, &'static str)> {
    let mut files = agents();
    files.extend([
        (
            ".claude/skills/oya-browser/SKILL.md",
            include_str!("team/skills/oya-browser/SKILL.md"),
        ),
        (
            ".claude/skills/oya-browser/assets/browser-check.md",
            include_str!("team/skills/oya-browser/assets/browser-check.md"),
        ),
        (
            ".claude/skills/team-work/SKILL.md",
            include_str!("team/skills/team-work/SKILL.md"),
        ),
        (
            ".claude/skills/team-work/assets/brief.md",
            include_str!("team/skills/team-work/assets/brief.md"),
        ),
        (
            ".claude/skills/team-work/assets/design.md",
            include_str!("team/skills/team-work/assets/design.md"),
        ),
        (
            ".claude/skills/team-work/assets/findings.md",
            include_str!("team/skills/team-work/assets/findings.md"),
        ),
        (
            ".claude/skills/team-work/assets/technical-plan.md",
            include_str!("team/skills/team-work/assets/technical-plan.md"),
        ),
        (
            ".claude/skills/team-work/assets/verification.md",
            include_str!("team/skills/team-work/assets/verification.md"),
        ),
        (
            ".claude/skills/team-work/references/designer.md",
            include_str!("team/skills/team-work/references/designer.md"),
        ),
        (
            ".claude/skills/team-work/references/pm.md",
            include_str!("team/skills/team-work/references/pm.md"),
        ),
        (
            ".claude/skills/team-work/references/principal.md",
            include_str!("team/skills/team-work/references/principal.md"),
        ),
        (
            ".claude/skills/team-work/references/qa.md",
            include_str!("team/skills/team-work/references/qa.md"),
        ),
        (
            ".claude/skills/team-work/references/reliability.md",
            include_str!("team/skills/team-work/references/reliability.md"),
        ),
        (
            ".claude/skills/team-work/references/reviewer.md",
            include_str!("team/skills/team-work/references/reviewer.md"),
        ),
        (
            ".claude/skills/team-work/references/security.md",
            include_str!("team/skills/team-work/references/security.md"),
        ),
    ]);
    files.extend_from_slice(bundled_skills::FILES);
    files
}

pub const OYA_BROWSER_AGENT: &str = include_str!("team/agents/oya-browser.md");

/// Where [`FRONTEND_SKILL`] is written. Only into a project with a frontend: a skill whose
/// description matches nothing the project does is noise in every turn's skill list.
pub const FRONTEND_SKILL_PATH: &str = ".claude/skills/frontend/SKILL.md";

/// Where [`ESLINT_RULES`] is written. Only into a project with a `package.json`.
pub const ESLINT_RULES_PATH: &str = "tooling/eslint/index.js";

/// [`RULES`] as lint rules, so the agent is held to them by the gate rather than by reading.
/// agentchrome's `tooling/eslint`, verbatim but for the header: no dependencies of its own, so it
/// can be written before anything is installed, and the project's ESLint config imports from it.
pub const ESLINT_RULES: &str = include_str!("team/eslint.js");

/// The heading [`RULES`] opens with.
pub const RULES_HEADING: &str = "## Engineering rules";

/// `claude_md` with the lifecycle and the engineering rules appended, each only if it is not
/// already there. Idempotent, so a scaffold, a pack that replaces the scaffold's `CLAUDE.md`, and
/// a second adoption can all call it.
pub fn with_guidance(claude_md: &str) -> String {
    let mut out = claude_md.trim_end().to_string();
    for (heading, section) in [(LIFECYCLE_HEADING, LIFECYCLE), (RULES_HEADING, RULES)] {
        if !out.to_lowercase().contains(&heading.to_lowercase()) {
            out = format!("{out}\n\n{section}");
        }
    }
    if out.trim_end() == claude_md.trim_end() {
        return claude_md.to_string();
    }
    out
}

pub const RULES: &str = r#"## Engineering rules

Defaults Keel starts every project with. Where the linter or another section of this file says
otherwise, that wins — edit these to match rather than keeping two answers. `principal` holds
changes to them; say which rule a deviation breaks and why it is worth it.

**Code quality**

- **Every file opens with a comment** saying what it is for. Every public function, type and
  route has a doc comment that says what it is for and why — what the decision was and what breaks
  without it — never the name again.
- **Small units.** Functions around 10 lines of logic, UI components around 50 with their logic in
  hooks or a view model, classes and modules around 200. Long code is steps that want names.
- **No magic numbers.** Each number gets a name in the folder's constants file; HTTP statuses come
  from one shared table; a value the environment can tune is read in one place with a named
  default.
- **No new dependency** for something a few lines of the standard library cover.
- **In JavaScript and TypeScript the linter holds these.** `tooling/eslint/index.js` has the rule
  sets (file header, function and class length, no magic numbers, doc comments); import them into
  the project's ESLint config rather than restating them.

**Design**

- **One folder per domain, one job per file.**
- **Facades.** Use another module through its entry file (`index.ts`, `mod.rs`, `__init__.py`),
  never its internals. Keep exported names stable; when code moves, re-export it from where it was.
- **Command maps instead of `switch` chains** — dispatch on a type through a map of handlers, and
  guard the lookup.
- **Strategies** for interchangeable implementations (providers, transports, stores): one
  interface, and the implementation picked in one place.
- **Repositories.** Services never touch the filesystem or the database directly.
- **Composition root.** Services receive their dependencies through their constructors, and one
  place builds them.
- **Thin edges.** A service raises a typed error carrying its status; routes, handlers and CLI
  commands translate and stay thin.

**Tests**

- Every change leaves tests behind. Unit tests mirror the source tree and are named after the file
  they test; an integration test when the change is a flow across modules.
- Test behaviour through the public surface, one test per rule, named after the rule — and the
  failure paths as well as the happy one.
- Tests are hermetic: no network, no real `.env`, state in a scratch directory, fakes at the seams,
  fake timers for anything time-based.
- Coverage never goes down.
"#;

pub const LIFECYCLE: &str = r#"## How a request becomes a change

Every request that changes the product runs this pipeline, in this order, and the person sees one
report at the end of it — not a stream of half-decisions along the way. The stages are the agents
in `.claude/agents/`. They run **one after another**, never in parallel, because each one reads
what the last one returned; and what a stage returned is handed to the next **verbatim**, because
a subagent starts with an empty context and a paraphrase is where a scope quietly grows.

1. **`pm`** — given the ask in the person's own words. Returns build / change / cut / not yet,
   and the smallest version worth shipping with its out-list. *Build* and *change* carry on, to
   the scope the PM set. **`cut` and `not yet` stop the pipeline and go back to the person**, with
   the reasoning. They can overrule it in one word.
2. **`designer`** — given the ask and the PM's scope, whenever anything a person reads changes: a
   page, a label, an error message, CLI output, an API response a client renders. Returns the
   states table, the layout, the copy. Skipped — and said to be skipped — when nothing visible
   moves.
3. **`principal`** — given the ask, the scope and the design. Reviews both for what they missed,
   then returns the technical design: the modules touched, what is reused, the invariants at risk,
   and the tests that will pin it. If the scope or the design cannot stand, that goes back through
   the stage that owns it once, not around it.
4. **Implementation** — the main loop, not a subagent, so the work is visible as it is written.
   It follows the principal's design; a deviation is written down with its reason, not made
   silently, and it keeps to "Engineering rules" below. Done means the gate is green, then
   `reviewer` on the diff — and `security` or
   `reliability` when their descriptions say the change is theirs.
5. **`qa`** — given the ask, the scope and what changed. *Do not ship* means fix and run `qa`
   again, **twice at most**; after that the person gets the change and the open bug list rather
   than a loop with no end.

**Direct mentions:** When the person tags a member, give that member the requested task directly;
do not require unrelated pipeline stages. `oya-browser` is the browser specialist for research,
web interaction, screenshots and browser QA. Use Oya exclusively for all browser work; if its
connection is unavailable, report the blocker and never substitute another provider. The team
ships role playbooks, specialist engineering skills, and Oya research with supporting assets.
Use `/handoff` when a session transfer is requested. Load `writing-for-agents` when editing
agent instructions or skills; these workflows do not change a member's permissions.

**What the person is shown** is one report: what was asked, what the PM scoped in and out, what
the designer and the principal decided in a line each, the files changed, the gate's result, QA's
verdict with what it covered and did not, and anything any stage was overruled on. A stage that
could not run is named as not run, never implied.

**What does not go through it:** a question, an explanation, running something, git and release
chores, edits to docs and to these agents, and a change with no decision in it — a typo, a rename,
a version bump. Say in one line that it was skipped. "Just do it" and "skip the pipeline" skip it;
naming stages ("no PM", "QA only") runs the ones named.
"#;

/// Keel's own, not a copy of Anthropic's `frontend-design`: that one ships under its own licence
/// and asks for boldness alone. This keeps its best idea — commit to a direction, never the
/// generic AI look — and adds what a product team needs next to it: the project's existing system
/// first, every state, accessibility, and looking at the result before calling it done.
pub const FRONTEND_SKILL: &str = r#"---
name: frontend
description: "Use when building or changing anything in the frontend: a page, a component, a layout, styling, copy on screen. Produces distinctive, production-grade UI that fits this project's existing design, covers every state, is accessible, and is checked in a browser before it is called done."
---

# Frontend

Build interfaces that look designed for this product, work in every state, and are verified by
looking at them.

## 1. Read before drawing

- Find the project's design system before inventing one: tokens (CSS variables, Tailwind config,
  theme file), the component library, how an existing page of the same kind is built. Grep for
  the pattern you are about to write; reuse it. Two ways to draw a button is one too many.
- Read `CLAUDE.md` for who the users are. A dashboard for engineers wants density; a landing page
  wants one idea per screen.

## 2. Commit to a direction

When there is no system yet, or the ask is a new surface, choose a clear aesthetic direction and
execute it with precision — refined minimalism and bold maximalism both work; intentionality is
the point. Never the generic AI look:

- choose typography for the product; preserve established app fonts and use native system fonts
  where appropriate; introduce a display face only when the content benefits from one
- no purple-to-blue gradient on white, no evenly spread timid palette: one dominant colour, sharp
  accents, and colour reserved for meaning
- no card grid because it is the default; compose for the content — asymmetry, overlap, generous
  space or deliberate density
- colours, spacing, radii and type sizes are tokens (CSS variables), never literals scattered
  through components

## 3. Every state, not the happy one

For each surface, design and build: empty, loading, partial, error (with what to do next),
permission denied, offline or stale, and far more data than expected (long names, 10,000 rows).
A blank area with no reason is a bug. Skeletons match the final layout so nothing jumps.

## 4. Accessible by default

Semantic elements first (`button`, `a`, `label`, `nav`, headings in order). Everything works from
the keyboard with visible focus. Text contrast at least 4.5:1 in light and dark. Images have alt
text; icon-only buttons have labels. Honour `prefers-reduced-motion` and `prefers-color-scheme`.

## 5. Motion and performance

Motion carries information — an arrival, a change, a relationship — or it goes. Prefer CSS
transitions; animate `transform` and `opacity` only. Nothing animates between a person and the
thing they are waiting for. Images are sized and lazy below the fold; fonts are subset and
`font-display: swap`; no layout shift when data lands.

## 6. Responsive

Check the supported minimum width and a comfortable desktop width with no accidental horizontal
scroll. For responsive websites, check 375, 768 and 1440px. Use at least 44px touch targets on
touch interfaces; keep desktop controls consistent with the existing application.

## 7. Look at it

Read `.claude/skills/oya-browser/SKILL.md` and use Oya Browser exclusively. If Oya is unavailable,
report the blocker without substituting another browser or harness. Run the dev server and open
the page in an owned Oya tab. Take a screenshot at each width and in each theme and look
at it; read the console for errors and warnings. Check the empty and error states by forcing
them. A UI change is not done until you have seen it render — the type checker cannot see a
button that is white on white.

## Report

Say which direction you chose and why, the states you built, what you verified by looking (with
the widths and themes), and anything you could not check.
"#;

pub const PM_AGENT: &str = include_str!("team/agents/pm.md");

pub const DESIGNER_AGENT: &str = include_str!("team/agents/designer.md");

pub const PRINCIPAL_AGENT: &str = include_str!("team/agents/principal.md");

pub const QA_AGENT: &str = include_str!("team/agents/qa.md");

#[cfg(test)]
mod tests {
    use super::*;

    /// Claude Code decides whether to delegate from `description` alone, and a description with
    /// a colon in it is a nested mapping unless it is quoted — an agent with no description is
    /// one that is never called. `name` must match the file, or two agents can collide.
    #[test]
    fn every_agent_loads_under_its_own_name() {
        let all = agents();
        assert_eq!(all.len(), 8);
        for (path, body) in all {
            let stem = path
                .strip_prefix(".claude/agents/")
                .and_then(|p| p.strip_suffix(".md"))
                .expect("agents live in .claude/agents");
            assert!(
                body.starts_with(&format!("---\nname: {stem}\n")),
                "{path}: name must match the file"
            );
            let front = body.split("\n---\n").next().unwrap();
            let desc = front
                .lines()
                .find_map(|l| l.strip_prefix("description: "))
                .unwrap_or_else(|| panic!("{path}: no description"));
            assert!(
                !desc.contains(": ") || (desc.starts_with('"') && desc.ends_with('"')),
                "{path}: a description with a colon must be quoted"
            );
        }
    }

    /// The lifecycle names every stage by the agent that runs it; a stage whose agent is not
    /// shipped is an instruction the main agent cannot follow.
    #[test]
    fn the_lifecycle_calls_only_agents_the_team_ships() {
        for name in [
            "pm",
            "designer",
            "principal",
            "qa",
            "oya-browser",
            "reviewer",
            "security",
            "reliability",
        ] {
            assert!(
                LIFECYCLE.contains(&format!("`{name}`")),
                "{name} not in lifecycle"
            );
            assert!(
                agents()
                    .iter()
                    .any(|(p, _)| p.ends_with(&format!("/{name}.md")))
            );
        }
    }

    #[test]
    fn guidance_is_added_once() {
        let once = with_guidance("# demo\n");
        assert!(once.starts_with("# demo\n\n## How a request becomes a change"));
        assert_eq!(once.matches(RULES_HEADING).count(), 1);
        assert_eq!(with_guidance(&once), once);
        // A project that already wrote its own, in its own capitalisation, keeps it untouched,
        // and gets only the section it lacks.
        let own =
            "# x\n\n## How A Request Becomes A Change\n\nours\n\n## Engineering Rules\n\nours";
        assert_eq!(with_guidance(own), own);
        let half = with_guidance("# x\n\n## engineering rules\n\nours");
        assert!(half.contains(LIFECYCLE_HEADING) && !half.contains(RULES_HEADING));
    }

    #[test]
    fn the_frontend_skill_loads_under_its_directory_name() {
        assert!(FRONTEND_SKILL.starts_with("---\nname: frontend\ndescription: \""));
        assert!(FRONTEND_SKILL_PATH.ends_with("/frontend/SKILL.md"));
        assert!(DESIGNER_AGENT.contains(FRONTEND_SKILL_PATH));
    }

    /// The principal is the stage that enforces the rules; if it stops naming them, they are a
    /// section nobody reads.
    #[test]
    fn the_principal_holds_changes_to_the_rules() {
        assert!(PRINCIPAL_AGENT.contains("\"Engineering rules\""));
        assert!(LIFECYCLE.contains("\"Engineering rules\""));
    }
}
