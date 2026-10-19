## How a request becomes a change

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
