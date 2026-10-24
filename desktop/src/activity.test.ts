// Display activity must track current work without changing the execution state used by queues.
import { describe, expect, it } from "vitest";
import { activity, waiting, workingMembers } from "./activity";
import type { Lane } from "./store";

const lane = (over: Partial<Lane> = {}): Lane => ({ id: "l", project: "/r", title: "Agy", agent: "antigravity", known: false, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [], ...over });

describe("observed activity", () => {
  it("native Codex attention takes priority and counts each waiting lane once", () => {
    const l = lane({ agent: "codex", running: true, attention: { id: "a", kind: "permission" } });
    expect(activity(l)).toEqual({ label: "waiting for you", tone: "warn" });
    expect(waiting(l)).toBe(1);
    expect(waiting({ ...l, pending: [{} as Lane["pending"][number]] })).toBe(1);
    expect(activity({ ...l, attention: undefined }).tone).toBe("live");
  });
  it("keeps a child-only lane working and animates only its named members", () => {
    const l = lane({ observation: { running: false, background: { a: "pm", b: null }, truncated: false } });
    expect(activity(l)).toEqual({ label: "pm and an agent working in the background", tone: "live" });
    expect(workingMembers(l)).toEqual(["pm"]);
    expect(l.running).toBe(false);
  });

  it("an idle observation overrides stale parent and child replay activity", () => {
    const l = lane({ running: true, conv: { turns: [], background: { old: "qa" } }, observation: { running: false, background: {}, truncated: false } });
    expect(activity(l).tone).toBe("idle");
    expect(workingMembers(l)).toEqual([]);
  });

  it("shows automatic parent work even when its settled transcript is blocked", () => {
    expect(activity(lane({ observation: { running: true, background: {}, truncated: false } }))).toMatchObject({ tone: "live", label: "Thinking…" });
  });

  it("permission prompts take precedence while independent members keep working", () => {
    const l = lane({ pending: [{} as Lane["pending"][number]], observation: { running: true, background: { a: "pm" }, truncated: false } });
    expect(activity(l)).toEqual({ label: "waiting for you", tone: "warn" });
    expect(workingMembers(l)).toEqual(["pm"]);
  });

  it.each(["claude", "codex"] as const)("preserves %s parent and background activity without observations", (agent) => {
    expect(activity(lane({ agent, running: true })).tone).toBe("live");
    expect(activity(lane({ agent, conv: { turns: [], background: { a: "qa" } } })).tone).toBe("live");
  });
});
