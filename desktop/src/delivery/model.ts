// Delivery facts carry immutable checkout identities so old passes cannot grade newer contents.

/** HEAD alone misses edits at the same commit; both fields identify the code that was checked. */
export interface CheckoutIdentity { head: string; tree: string }
/** Captures belong to execution time; absent or unequal before/after identities remain unconfirmed. */
export interface CheckEvidence {
  id: string; status: string; source: string; command: string | null; plan: string;
  started_at: number; completed_at: number | null; before: CheckoutIdentity | null;
  after: CheckoutIdentity | null; successor: string | null;
  warning?: string | null;
  problems: { file: string; line: number; col: number; message: string }[];
}

/** Recovery names the exact existing lane and checkout rather than creating a replacement. */
export interface Owner { lane: string; provider: "claude" | "codex" | "antigravity" | "opencode"; session: string; checkout: string; wt?: string | null }
/** Historical anchors never move to an invented current line after edits. */
export interface Anchor { identity: CheckoutIdentity; path: string; side: "old" | "new" | "file"; first: number | null; last: number | null; quote: string }
/** Resolution stays independent from delivery and agent completion. */
export interface Correction { id: string; revision: number; source: string; body: string; url?: string | null; resolution: "unresolved" | "human_resolved" | "remote_resolved"; remote_open?: boolean | null; outdated: boolean; deliveries: string[]; anchor?: Anchor | null }
/** One editable draft stays separate from captured publication requests. */
export interface PrForm { title: string; body: string; draft: boolean; base: string; edited: boolean; seeded: boolean }
/** Requested and observed publication commits can differ after another writer's push. */
export interface Publication { receipt: string; intent: { identity: CheckoutIdentity; branch: string; base: string; repository: string }; status: string; url?: string | null; actual_head?: string | null; error?: string | null }
/** Source material remains readable across provider transfers. */
export interface Source { kind: string; snapshot?: { title: string; url: string; body: string; fetched_at: number; repository: string; number: number }; name?: string; revision?: number; instruction?: string }
/** Project history never enables recurring execution. */
export interface Record { task: string; revision: number; title: string; source: Source; owner: Owner; base: string; branch: string; checks: CheckEvidence[]; corrections: Correction[]; form: PrForm; reviews?: { id: string; sha: string; base: string; provider: string; verdict: string; report: string; confirmed: boolean; completed_at?: number | null }[]; publication?: Publication | null; feedback?: { head: string; fetched_at: number; complete: boolean; errors: string[] } | null }
/** Completion, project checks and advisory review remain separate facts. */
export interface TaskView { record: Record; owner?: Owner | null; owner_error?: string | null; identity?: CheckoutIdentity | null; checks_current: boolean; checks_passed: boolean; evidence_error?: string | null; control_error?: string | null; operations?: Receipt[] }
/** Discovery works without a saved window lane. */
export interface TaskSummary { task: string; title: string; source: string; source_url?: string | null; owner: Owner; unresolved: number; remote_open: number; check?: CheckEvidence | null; publication?: Publication | null; managed: boolean; lanes?: string[] }
/** Keyset pages preserve strict ownership failures as visible coverage limits. */
export interface TaskPage { tasks: TaskSummary[]; next?: string | null; owner_error?: string | null }
/** Previewing never starts a checkout or coding agent. */
export interface IssuePreview { id: string; issue: { title: string; body: string; url: string; state: string; truncated: boolean }; change_repository: string; parent?: string | null }
/** A durable receipt preserves uncertain operations without resending them. */
export interface Receipt { id: string; task?: string | null; kind: string; phase: string; error?: string | null; target?: { head: string; branch: string; base: string; repository: string } | null; result?: { owner?: Owner; task?: string } | null }
/** Intervals and run limits are seconds; every new definition begins paused. */
export interface Schedule { id: string; revision: number; name: string; prompt: string; provider: Owner["provider"]; base: string; interval: number; duration: number; enabled: boolean; next_at?: number | null; error?: string | null }
/** Uncertain execution retains its project slot until explicit reconciliation. */
export interface Occurrence { id: string; schedule: string; task?: string | null; phase: string; due_at: number; reason?: string | null }
/** Bounded history distinguishes skipped ticks and active occurrences. */
export interface SchedulePage { schedules: Schedule[]; occurrences: Occurrence[]; active?: string | null; active_occurrence?: Occurrence | null; next?: string | null; controller_error?: string|null }
