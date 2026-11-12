import { describe, expect, it } from "vitest";
import { applyFact, applyFrames, restoreTranscript, interrupt, type Conversation, type Frame } from "./reduce";

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

describe("terminal history handoff", () => {
  it("keeps a confirmed completion when provider history omits its end marker", () => {
    const previous = applyFrames(empty, [f("open", { prompt: "first" }), f("close", { reason: "done" })]);
    const restored = restoreTranscript(previous, [f("open", { prompt: "first" }), f("close", { reason: "superseded" })]);
    expect(restored.turns[0].closed).toBe("done");
  });
  it("imports terminal messages without duplicating prior chat and preserves checkpoint metadata", () => {
    let previous = applyFrames(empty, [f("open", { prompt: "first" }, "chat-first"), f("text", { step: "a", kind: "say", append: "old" }), f("close", { reason: "done" })]);
    previous = { turns: previous.turns.map((t) => ({ ...t, snapshot: "tree-before", files: ["a.ts"] })) };
    const frames = [f("open", { prompt: "first" }, "native-first"), f("text", { step: "a", kind: "say", append: "old" }), f("close", { reason: "done" }), f("open", { prompt: "from terminal" }, "native-next"), f("text", { step: "b", kind: "say", append: "new" }), f("close", { reason: "done" })];
    const restored = restoreTranscript(previous, frames);
    expect(restored.turns.map((t) => t.prompt)).toEqual(["first", "from terminal"]);
    expect(restored.turns[0].snapshot).toBe("tree-before");
    expect(restored.turns[0].id).toBe(previous.turns[0].id);
    expect(restored.turns[1].snapshot).toBeUndefined();
    expect(restoreTranscript(restored, frames).turns).toEqual(restored.turns);
  });
});

describe("restoring a transcript over the live view", () => {
  const done = (prompt: string) => [f("open", { prompt }), f("text", { step: prompt, kind: "say", append: "ok" }), f("close", { reason: "done" })];
  it("keeps a turn refused before it started, and matches the turns after it", () => {
    let previous = applyFrames(empty, [...done("one"), f("open", { prompt: "refused" }), f("close", { reason: "failed" }), ...done("two")]);
    previous = { turns: previous.turns.map((t) => ({ ...t, commit: `c-${t.prompt}` })) };
    const restored = restoreTranscript(previous, [...done("one"), ...done("two")]);
    expect(restored.turns.map((t) => t.prompt)).toEqual(["one", "refused", "two"]);
    expect(restored.turns[2].commit).toBe("c-two");
    expect(restored.turns[2].id).toBe(previous.turns[2].id);
  });
  it("matches a turn that carried attachments, however each side wrote them", () => {
    const shown = "look at this\n\nAttached files in this checkout (already shown to you above):\n.keel/attachments/1-shot.png";
    const previous = { turns: applyFrames(empty, done(shown)).turns.map((t) => ({ ...t, commit: "abc", files: ["a.ts"] })) };
    // The transcript: the text block, then an empty image block joined with a space.
    for (const written of [shown + " ", "look at this\n\nReferenced files in this checkout (JSON paths):\n[\"a.ts\"]\n\nAttached files in this checkout (already shown to you above):\n.keel/attachments/1-shot.png"]) {
      const restored = restoreTranscript(previous, done(written));
      expect(restored.turns[0].commit).toBe("abc");
      expect(restored.turns[0].id).toBe(previous.turns[0].id);
    }
    // A non-image file: the live view names nothing, the transcript adds its path as a block.
    const plain = { turns: applyFrames(empty, done("read the notes")).turns.map((t) => ({ ...t, commit: "def" })) };
    expect(restoreTranscript(plain, done("read the notes Attached file: /repo/.keel/attachments/2-notes.txt")).turns[0].commit).toBe("def");
  });
  it("keeps the same words sent with different screenshots apart", () => {
    const with_ = (shot: string) => `fix this\n\nAttached files in this checkout (already shown to you above):\n.keel/attachments/${shot}`;
    const previous = { turns: applyFrames(empty, [...done(with_("a.png")), ...done(with_("b.png"))]).turns.map((t, i) => ({ ...t, commit: `c${i}` })) };
    const restored = restoreTranscript(previous, done(with_("b.png") + " "));
    expect(restored.turns.at(-1)?.commit).toBe("c1");
  });
  it("keeps what only the live view knew: a failure, its error, the cost, the turns before the tail", () => {
    let previous = applyFrames(empty, [...done("early"), f("open", { prompt: "broke" }), f("text", { step: "x", kind: "say", append: "half" }), f("failure", { text: "quota reached" }), f("close", { reason: "failed" })]);
    previous = { turns: previous.turns.map((t) => ({ ...t, usage: { context: 0, input: 1, output: 1, cache_read: 0, cache_write: 0, cost: 0.25 } })) };
    // A tail read that starts at the second turn, whose history has no end marker or failure.
    const restored = restoreTranscript(previous, [f("open", { prompt: "broke" }), f("text", { step: "x", kind: "say", append: "half" }), f("close", { reason: "done" })]);
    expect(restored.turns.map((t) => t.prompt)).toEqual(["early", "broke"]);
    expect(restored.turns[1].closed).toBe("failed");
    expect(restored.turns[1].failures).toEqual(previous.turns[1].failures);
    expect(restored.turns[1].usage?.cost).toBe(0.25);
  });
  it("matches a prompt the daemon sent with its referenced files", () => {
    const previous = { turns: applyFrames(empty, done("change this")).turns.map((t) => ({ ...t, gate: { status: "passed" } })) };
    const restored = restoreTranscript(previous, done('change this\n\nReferenced files in this checkout (JSON paths):\n["a.ts"]'));
    expect(restored.turns[0].gate?.status).toBe("passed");
    expect(restored.turns[0].prompt).toBe("change this");
  });
});

describe("background agents", () => {
  it("work across turns until their notification, and survive a fact", () => {
    let c = applyFrames(empty, [
      f("open", { prompt: "review" }, "u1"),
      f("background", { id: "c1", agent: "pm", running: true }, "u1"),
      f("close", { reason: "done" }, "u1"),
    ]);
    expect(c.background).toEqual({ c1: "pm" });
    c = applyFact(c, { turn: "u1", kind: "turn.ended" });
    expect(c.background).toEqual({ c1: "pm" });
    c = applyFrames(c, [f("background", { id: "c1", running: false })]);
    expect(c.background).toEqual({});
  });
});

describe("review fixes", () => {
  const done = (prompt: string) => [f("open", { prompt }), f("close", { reason: "done" })];
  it("a prompt resent after a refusal keeps its own turn", () => {
    let previous = applyFrames(empty, [f("open", { prompt: "fix it" }), f("close", { reason: "failed" }), ...done("fix it")]);
    previous = { turns: previous.turns.map((t, i) => (i === 1 ? { ...t, commit: "abc" } : t)) };
    const restored = restoreTranscript(previous, done("fix it"));
    expect(restored.turns.map((t) => t.closed)).toEqual(["failed", "done"]);
    expect(restored.turns[1].commit).toBe("abc");
  });
  it("a close with nothing open draws no turn", () => {
    expect(applyFrames(applyFrames(empty, done("a")), [f("close", { reason: "interrupted" })]).turns).toHaveLength(1);
  });
  it("closing a turn left open keeps the background agents", () => {
    const conv = { ...applyFrames(empty, [f("open", { prompt: "a" })]), background: { x: "agent" } };
    expect(interrupt(conv).background).toEqual({ x: "agent" });
  });
});

describe("notices", () => {
  it("a notice between turns joins the turn on screen, not a new one", () => {
    const c = applyFrames(applyFrames(empty, [f("open", { prompt: "a" }), f("close", { reason: "done" })]), [f("notice", { text: "The conversation was compacted" })]);
    expect(c.turns).toHaveLength(1);
    expect(Object.values(c.turns[0].blocks).map((b) => b.kind)).toEqual(["notice"]);
  });
});
