// A chat turn's footer, from the usage the agent reported.
import type { Usage } from "./reduce";

/** The turn's footer as `/cost` and the status line say it: how long, how much was read and
 *  written, what it cost, and how full the context is. Only what the agent reported. */
export function turnFacts(usage: Usage | undefined, ms: number | undefined): string[] {
  const out: string[] = [];
  if (ms) out.push(clock(ms));
  if (usage?.output) out.push(`${tokens(usage.input + usage.cache_read + usage.cache_write)} in · ${tokens(usage.output)} out`);
  if (typeof usage?.cost === "number") out.push(`$${usage.cost.toFixed(usage.cost < 1 ? 3 : 2)}`);
  if (usage?.context) out.push(`${tokens(usage.context)} context`);
  return out;
}
const tokens = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k` : String(n));

/** " in 1m 23s", or nothing when the agent reported no time. */
export function duration(ms: number | undefined): string {
  if (!ms) return "";
  return ` in ${clock(ms)}`;
}

/** Seconds rounded first, so 119.7 s reads "2m 0s" and never "1m 60s". */
function clock(ms: number): string {
  const seconds = Math.round(ms / 1000);
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}
