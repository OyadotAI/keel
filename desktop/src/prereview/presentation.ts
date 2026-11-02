// Presentation derives labels from confirmed ownership; it never changes server or delivery state.
import type { Review, Snapshot } from "./model";
import { clock } from "./format";
import { prForTarget, type PrRun } from "./opening";

/** Last-known results stay readable while their color and actions reflect missing confirmation. */
export function verdictPresentation(review: Review, outcome: Extract<NonNullable<Review["outcome"]>, { kind: "verdict" }>, head: string, snapshot?: Snapshot) {
  const sent = snapshot?.delivery?.id === review.id;
  const moved = !!head && !!review.sha && head !== review.sha;
  const unconfirmed = snapshot?.confirmed === false, ship = outcome.verdict === "ship";
  const title = `${ship ? "Ship" : "Do not ship"}${unconfirmed ? " — previous result, status unconfirmed" : ""}`;
  const icon = unconfirmed ? "minus-circle" as const : ship ? "check-circle" as const : "hand" as const;
  const color = unconfirmed ? "var(--dim)" : ship ? "var(--add)" : "var(--warn)";
  const delivery = sent ? snapshot.delivery?.state === "queued" ? "Findings queued for the agent." : "Findings sent to the agent." : undefined;
  const meta = `${review.sha.slice(0, 7)} vs ${review.base} · ${clock(outcome.took)}${outcome.cost ? ` · $${outcome.cost.toFixed(2)}` : ""}`;
  return { sent, moved, unconfirmed, ship, stale: sent || moved || unconfirmed, title, icon, color, delivery, meta };
}

/** A changed branch gets its own form while a still-running publication retains its owner. */
export function formPresentation(snapshot: Snapshot | undefined, opening: { state: PrRun; running: boolean } | undefined, branch?: string | null) {
  const current = snapshot?.target?.branch || branch || "Current branch";
  const base = snapshot?.target?.base || snapshot?.review?.base || "base checked when review starts";
  const run = prForTarget(opening, snapshot?.target?.branch ?? branch ?? undefined);
  const pending = !!snapshot?.operation || !!run?.running;
  return { branch: current, base, run, pending, review: snapshot?.review, hasResult: !!snapshot?.review || !!snapshot?.restarted || !!run };
}
