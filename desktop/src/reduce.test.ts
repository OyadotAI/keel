import { describe, expect, it } from "vitest";
import { applyFact, applyFrames, interrupt, type Conversation, type Frame } from "./reduce";

const empty: Conversation = { turns: [] };
const f = (op: string, fields: Record<string, unknown> = {}, turn: string | null = null): Frame => ({ turn, op, ...fields });

describe("folding a turn", () => {
  it("appends text to its step and keeps steps in order", () => {
    const c = applyFrames(empty, [
      f("open", { prompt: "ask" }),
      f("text", { step: "m:0", kind: "say", append: "Hel" }),
      f("call", { id: "c1", tool: "Bash", subject: "ls", writes: [] }),
      f("text", { step: "m:0", kind: "say", append: "lo" }),
      f("result", { id: "c1", state: "ok", output: "a" }),
    ]);
    const t = c.turns[0];
    expect(t.steps).toEqual(["b:m:0", "c:c1"]);
    expect(t.blocks["m:0"].text).toBe("Hello");
    expect(t.calls.c1.state).toBe("ok");
  });

  it("touches only the block a delta lands in", () => {
    const a = applyFrames(empty, [f("open", { prompt: "x" }), f("text", { step: "s1", kind: "say", append: "a" }), f("text", { step: "s2", kind: "say", append: "b" })]);
    const b = applyFrames(a, [f("text", { step: "s2", kind: "say", append: "c" })]);
    expect(b.turns[0].blocks.s1).toBe(a.turns[0].blocks.s1);
    expect(b.turns[0].blocks.s2).not.toBe(a.turns[0].blocks.s2);
  });

  it("leaves no call running after close, or after a stream that never closed", () => {
    const open = applyFrames(empty, [f("open", { prompt: "x" }), f("call", { id: "c", tool: "Bash", subject: "", writes: [] })]);
    expect(applyFrames(open, [f("close", { reason: "stopped" })]).turns[0].calls.c.state).toBe("interrupted");
    expect(interrupt(open).turns[0].calls.c.state).toBe("interrupted");
  });

  it("replaces a turn sent again on a resumed replay instead of duplicating it", () => {
    const once = applyFrames(empty, [f("open", { prompt: "x" }, "u1"), f("text", { step: "s", kind: "say", append: "a" }, "u1")]);
    const twice = applyFrames(once, [f("open", { prompt: "x" }, "u1"), f("text", { step: "s", kind: "say", append: "a" }, "u1")]);
    expect(twice.turns).toHaveLength(1);
    expect(twice.turns[0].blocks.s.text).toBe("a");
    expect(twice.turns[0].id).toBe(once.turns[0].id);
  });

  it("files a keyless chat turn's facts once the key arrives", () => {
    let c = applyFrames(empty, [f("open", { prompt: "x" })]);
    c = applyFact(c, { turn: "u9", kind: "turn.files", files: ["a.ts"] });
    c = applyFact(c, { turn: "u9", kind: "turn.gate", status: "passed" });
    expect(c.turns[0].key).toBe("u9");
    expect(c.turns[0].files).toEqual(["a.ts"]);
    expect(c.turns[0].gate?.status).toBe("passed");
  });

  it("files a chat turn's gate and commit, which arrive after it closed", () => {
    let c = applyFrames(empty, [f("open", { prompt: "x" })]);
    c = applyFact(c, { turn: "u9", kind: "approval.asked", id: "a" });
    c = applyFrames(c, [f("close", { reason: "done" })]);
    c = applyFact(c, { turn: "u9", kind: "turn.files", files: ["a.ts"] });
    c = applyFact(c, { turn: "u9", kind: "turn.gate", status: "passed" });
    c = applyFact(c, { turn: "u9", kind: "turn.ended" });
    expect(c.turns[0]).toMatchObject({ key: "u9", files: ["a.ts"], gate: { status: "passed" }, ended: true });
  });

  it("files a late keyed fact on a chat turn that never heard its key before closing", () => {
    let c = applyFrames(empty, [f("open", { prompt: "x" }), f("close", { reason: "done" })]);
    c = applyFact(c, { turn: "u1", kind: "turn.commit", sha: "abc" });
    expect(c.turns[0].commit).toBe("abc");
  });

  it("keeps the snapshot a turn can be rewound to, and how long it took", () => {
    let c = applyFrames(empty, [f("open", { prompt: "x" })]);
    c = applyFact(c, { turn: "u", kind: "turn.started", started: "2026-10-05T10:00:00Z", snapshot: "abc123", prompt: "x" });
    c = applyFact(c, { turn: "u", kind: "turn.ended", ended: "2026-10-05T10:01:00Z", ms: 60000 });
    expect(c.turns[0]).toMatchObject({ snapshot: "abc123", ms: 60000, ended: true });
  });

  it("nests a subagent's calls under the call that started it", () => {
    const c = applyFrames(empty, [
      f("open", { prompt: "x" }),
      f("call", { id: "task", tool: "Task", subject: "", writes: [] }),
      f("call", { id: "inner", tool: "Read", subject: "a", writes: [], parent: "task" }),
    ]);
    expect(c.turns[0].steps).toEqual(["c:task"]);
    expect(c.turns[0].children.task).toEqual(["inner"]);
  });
});
