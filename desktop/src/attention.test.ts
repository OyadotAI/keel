// Attention contracts prevent acknowledgement, replay and unrelated lanes from hiding live work.
import { describe, expect, it } from "vitest";
import { attentionCoverage, attentionEnding, attentionItems, attentionSelection, type AttentionSource } from "./attention";
import type { Lane, Pending } from "./store";
import { applyFrames, type Turn } from "./reduce";

const lane = (id: string, over: Partial<Lane> = {}): Lane => ({ id, project: "/a", title: id, agent: "claude", known: true, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [], ...over });
const pending = (id = "request", tool = "AskUserQuestion"): Pending => ({ id, tool, lane: "q", session_id: "session", command: "", input: {}, rules: [] });
const turn = (over: Partial<Turn> = {}): Turn => ({ ...applyFrames({ turns: [] }, [{ op: "open", turn: "t", prompt: "Fix checkout" }]).turns[0], closed: "done", ended: true, ...over });
function source(lanes: Lane[]): AttentionSource {
  return { order: ["/a", "/b"], projects: { "/a": { name: "A", lanes: lanes.filter((l) => l.project === "/a").map((l) => l.id) }, "/b": { name: "B", lanes: lanes.filter((l) => l.project === "/b").map((l) => l.id) } }, lanes: Object.fromEntries(lanes.map((l) => [l.id, l])) };
}

describe("workspace attention", () => {
  it("prioritizes input, errors, latest failed checks and updates across projects in stable order", () => {
    const s = source([lane("update", { unseen: true }), lane("check", { conv: { turns: [turn({ gate: { status: "failed" } })] } }), lane("error", { error: "offline" }), lane("q-a", { pending: [pending()] }), lane("q-b", { project: "/b", pending: [pending()] })]);
    expect(attentionItems(s).map((row) => row.lane)).toEqual(["q-a", "q-b", "error", "check", "update"]);
  });

  it("counts one lane once while preserving every independent reason and both recovery errors", () => {
    const s = source([lane("all", { pending: [pending(), pending("two", "Bash")], attention: { id: "native", kind: "permission" }, error: "connection lost", nameError: "name failed", conv: { turns: [turn({ gate: { status: "failed" } })] }, unseen: true })]);
    const [row] = attentionItems(s);
    expect(attentionItems(s)).toHaveLength(1);
    expect(row.reasons.map((reason) => reason.kind)).toEqual(["input", "error", "error", "checks", "update"]);
    expect(row.destination).toEqual({ kind: "request", request: "request" });
    expect(row.reasons[0].text).toContain("2 requests");
  });

  it("preserves unresolved conditions after the unseen update is acknowledged", () => {
    const s = source([lane("q", { unseen: true, pending: [pending()], error: "answer delivery failed", conv: { turns: [turn({ gate: { status: "failed" } })] } })]);
    const snapshot = JSON.stringify(s);
    attentionItems(s);
    expect(JSON.stringify(s)).toBe(snapshot);
    s.lanes.q = { ...s.lanes.q!, unseen: false };
    expect(attentionItems(s)[0].reasons.map((reason) => reason.kind)).toEqual(["input", "error", "checks"]);
  });

  it("includes live question delivery errors without exposing saved answers or closed drafts", () => {
    const request = pending();
    const s = source([lane("q", { pending: [request], questionDrafts: { request: { request, values: { 0: { custom: "private answer", mode: "custom", selected: [] } }, error: "Delivery unconfirmed" }, closed: { request: pending("closed"), values: {}, error: "old error" } } })]);
    const text = JSON.stringify(attentionItems(s));
    expect(text).toContain("Delivery unconfirmed");
    expect(text).not.toContain("private answer");
    expect(text).not.toContain("old error");
    s.lanes.q = { ...s.lanes.q!, pending: [] };
    expect(attentionItems(s)).toEqual([]);
  });

  it("routes native prompts to the agent terminal and permission cards to their existing control", () => {
    const [native, card] = attentionItems(source([lane("native", { attention: { id: "n", kind: "permission" } }), lane("card", { pending: [pending("p", "Bash")] })]));
    expect(native).toMatchObject({ action: "Open terminal", destination: { kind: "terminal" } });
    expect(card).toMatchObject({ action: "Review request", destination: { kind: "request", request: "p" } });
  });

  it("a newer turn supersedes older failed checks without inventing a successful check", () => {
    const s = source([lane("q", { conv: { turns: [turn({ gate: { status: "failed" } }), turn({ closed: undefined, ended: false })] } })]);
    expect(attentionItems(s)).toEqual([]);
    s.lanes.q = { ...s.lanes.q!, unseen: true };
    expect(attentionItems(s)[0].reasons).toEqual([{ kind: "update", text: "New update" }]);
  });

  it("filters removed, orphaned, duplicated and mismatched memberships without recreating rows", () => {
    const s = source([lane("q", { pending: [pending()] }), lane("orphan", { project: "/closed", error: "old" })]);
    s.projects["/a"]!.lanes.push("q", "missing", "orphan");
    s.projects["/b"]!.lanes.push("q");
    expect(attentionItems(s).map((row) => row.lane)).toEqual(["q"]);
    s.order = ["/b"];
    expect(attentionSelection(s, "q")).toBeUndefined();
  });

  it("keeps known rows under partial loading and qualifies disconnected sources", () => {
    const s = source([lane("q", { loaded: false, pending: [pending()], disconnected: true }), lane("idle", { loaded: false })]);
    expect(attentionCoverage(s)).toEqual({ projects: 2, unloaded: 2 });
    expect(attentionItems(s)[0].disconnected).toBe(true);
    s.lanes.q = { ...s.lanes.q!, disconnected: false };
    s.projects["/a"]!.error = "daemon stopped";
    expect(attentionItems(s)[0].disconnected).toBe(true);
  });
});

describe("ending evidence", () => {
  it.each([
    [turn(), "Finished · Not checked"],
    [turn({ commit: "abc" }), "Finished · Not checked"],
    [turn({ gate: { status: "passed" } }), "Finished · Checks passed"],
    [turn({ gate: { status: "running" } }), "Finished · Checks running"],
    [turn({ gate: { status: "future" } }), "Finished · Check status unavailable"],
    [turn({ gate: { status: "constructor" } }), "Finished · Check status unavailable"],
    [turn({ gate: { status: "toString" } }), "Finished · Check status unavailable"],
    [turn({ gate: { status: "__proto__" } }), "Finished · Check status unavailable"],
    [turn({ closed: "stopped", gate: { status: "passed" } }), "Stopped · Checks passed"],
    [turn({ closed: "interrupted" }), "Stopped · Not checked"],
    [turn({ closed: "failed", gate: { status: "passed" } }), "Did not finish · Checks passed"],
    [turn({ failures: [{ kind: "runtime", message: "failed", history: false }] }), "Did not finish · Not checked"],
    [turn({ closed: "future" }), "New update · Not checked"],
    [undefined, "New update"],
  ] as const)("keeps execution and check outcomes independent: %s", (t, expected) => expect(attentionEnding(t)).toBe(expected));
});
