// Reported usage replaces repeated snapshots and counts each disjoint input category once.
import { describe, expect, it } from "vitest";
import { applyFrames } from "./reduce";
import { usageSummary } from "./session-usage";
import type { Lane } from "./store";

const lane = (over: Partial<Lane> = {}): Lane => ({ id: "l", project: "/r", title: "Usage", agent: "claude", known: false, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [], ...over });
const frames = [{ turn: "t", op: "open" }, { turn: "t", op: "usage", context: 150, input: 100, cache_read: 50, cache_write: 20, output: 10 }];

describe("reported session usage", () => {
  it("counts cache-inclusive input once and keeps latest context separate", () => {
    const l = lane({ conv: applyFrames({ turns: [] }, frames) });
    expect(usageSummary(l)).toMatchObject({ input: 170, context: 150, reported: true, sum: { output: 10 } });
    expect(usageSummary(l).detail).toContain("Cache reads 50");
    expect(usageSummary(l).detail).toContain("snapshot, not cumulative usage");
  });

  it("repeated frames and replays replace usage instead of adding it", () => {
    const once = applyFrames({ turns: [] }, frames);
    const twice = applyFrames(once, frames);
    expect(usageSummary(lane({ conv: twice }))).toEqual(usageSummary(lane({ conv: once })));
  });

  it("observations replace transcript totals and disclose excluded child and earlier usage", () => {
    const l = lane({ agent: "antigravity", conv: applyFrames({ turns: [] }, frames), observation: { running: true, background: { child: "pm" }, usage: { context: 900, input: 700, cache_read: 200, cache_write: 0, output: 1616 }, truncated: true } });
    const summary = usageSummary(l);
    expect(summary).toMatchObject({ input: 900, context: 900, sum: { output: 1616 } });
    expect(summary.detail).toContain("Parent conversation only. Subagent usage is not included.");
    expect(summary.detail).toContain("totals cover the loaded history");
    expect(usageSummary({ ...l, observation: { ...l.observation!, usage: null } }).reported).toBe(false);
  });

  it("distinguishes missing usage from a provider reporting zero", () => {
    expect(usageSummary(lane()).reported).toBe(false);
    const l = lane({ observation: { running: false, background: {}, truncated: false, usage: { input: 0, output: 0, cache_read: 0, cache_write: 0 } } });
    expect(usageSummary(l)).toMatchObject({ reported: true, input: 0, sum: { output: 0 }, context: undefined });
    expect(usageSummary(l).detail).toContain("Input 0 · Output 0");
  });
});
