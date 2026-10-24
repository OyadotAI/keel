// Provider-normalized reported usage shared by the footer and Context panel. Snapshots replace
// replay totals; cached input is counted once, and context remains the latest occupancy sample.
import type { Lane } from "./store";
import { contextMatches } from "./context";

/** Supplemental display state cannot drive submission or turn completion. */
export interface Observation {
  running: boolean;
  at?: string | null;
  background: Record<string, string | null>;
  usage?: { context?: number | null; input: number; output: number; cache_read: number; cache_write: number } | null;
  truncated: boolean;
}

function parentTotals(l: Lane) {
  const sum = { input: 0, output: 0, cache_read: 0, cache_write: 0, turns: 0 };
  let context: number | undefined;
  let cost = 0;
  for (const t of l.conv.turns) {
    cost += t.usage?.cost ?? 0;
    if (!t.usage) continue;
    for (const k of ["input", "output", "cache_read", "cache_write"] as const) sum[k] += t.usage[k];
    sum.turns += 1;
    if (t.usage.context > 0) context = t.usage.context;
  }
  const observed = l.observation;
  if (observed) {
    Object.assign(sum, observed.usage ?? { input: 0, output: 0, cache_read: 0, cache_write: 0 });
    context = observed.usage?.context ?? undefined;
  }
  const reported = observed ? observed.usage != null : sum.turns > 0;
  const input = sum.input + sum.cache_read + sum.cache_write;
  return { context, input, sum, cost, reported };
}

function usageDetail(l: Lane, totals: ReturnType<typeof parentTotals>) {
  const { context, input, sum, cost, reported } = totals;
  return [
    "Reported usage",
    reported ? `Input ${input.toLocaleString()} · Output ${sum.output.toLocaleString()}` : "Usage has not been reported yet.",
    "In: Reported input tokens across this conversation, including cached input counted once.",
    "Out: Reported output tokens across this conversation.",
    ...(reported ? [`Fresh input ${sum.input.toLocaleString()}`, `Cache reads ${sum.cache_read.toLocaleString()}`, ...(l.agent === "claude" ? [`Cache writes ${sum.cache_write.toLocaleString()}`] : [])] : []),
    context != null ? `Latest context ${context.toLocaleString()}` : "Context size has not been reported.",
    "Context: Latest reported context size. This is a snapshot, not cumulative usage.",
    "Parent conversation only. Subagent usage is not included.",
    ...(l.observation?.truncated || l.historyTruncated ? ["Earlier turns are unavailable; totals cover the loaded history."] : []),
    "Updates when the agent reports usage; the current response may be incomplete.",
    "Reported tokens for this conversation; account usage and billing may differ.",
    ...(cost ? [`Cost $${cost.toFixed(4)}`] : []),
  ].join("\n");
}

/** Summarize the reported parent history without adding a snapshot to its replayed records. */
export function usageSummary(l: Lane) {
  const totals = parentTotals(l);
  let { context } = totals;
  const inspection = contextMatches(l.context, l.agent ?? "claude", l.known ? l.session : undefined) ? l.context : undefined;
  if (inspection?.used != null) context = inspection.used;
  else if (inspection?.compacted_at) context = undefined;
  const percentage = context != null && inspection?.capacity ? Math.round(context / inspection.capacity * 100) : undefined;
  const summary = { ...totals, context, percentage };
  return { ...summary, detail: usageDetail(l, summary) };
}
