// Review transport facts remain separate from the window's pending actions and editable form.
export interface Finding {
  path: string;
  line: number;
  severity: "blocking" | "warning" | "note" | string;
  title: string;
  body: string;
}

export type Outcome =
  | { kind: "verdict"; verdict: string; reason: string; summary: string; findings: Finding[]; report: string; took: number; cost?: number | null }
  | { kind: "none"; cause: string; line: string; took: number; cost?: number | null };

export interface Opening {
  title: string;
  body: string;
  draft: boolean;
  base: string;
}

export interface Review {
  id: string;
  revision: number;
  cancel_requested?: boolean;
  lane: string;
  wt?: string | null;
  sha: string;
  base: string;
  started: number;
  phase: "waiting" | "checking" | "fetching" | "reviewing" | "done";
  /// What Open will open: the person's title and draft, or the agent's `gh pr create` words.
  opening: Opening;
  by_agent: boolean;
  outcome?: Outcome | null;
}

export interface Seen {
  review: Review | null;
  /// Where the checkout stands now: a review of another commit is stale.
  head: string;
  /// The page saw it running and the daemon no longer has it: Keel restarted.
  restarted?: boolean;
  target?: { branch: string; base: string; head: string };
}


/** One pending owner prevents the tab and dock from issuing conflicting mutations. */
export type Operation = "start" | "cancel" | "dismiss" | "fix" | "open";
/** Edited input is independent of the metadata frozen into an earlier review. */
export interface ReviewForm { title: string; draft: boolean; edited: boolean }
/** Recovery keeps the operation and raw detail rather than hiding failure behind an idle button. */
export interface ReviewError { action: Operation | "read"; message: string; details: string }
/** Confirmation is revoked on read failure while useful prior results remain readable. */
export interface Snapshot extends Seen {
  context: string;
  generation: symbol;
  form: ReviewForm;
  confirmed: boolean;
  loading: boolean;
  operation?: Operation;
  error?: ReviewError;
  delivery?: { id: string; state: "sent" | "queued" };
}
/** A captured endpoint and checkout prevent delayed work from retargeting another conversation. */
export interface Scope { lane: string; ep: import("../api").Endpoint; wt?: string }
