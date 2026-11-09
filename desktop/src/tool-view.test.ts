// A call's input decides how it is drawn, and an unknown shape is never guessed at.
import { describe, expect, it } from "vitest";
import { describe as said, toolView } from "./tool-view";

describe("toolView", () => {
  it("draws an edit as its changed lines between shared context", () => {
    const v = toolView("Edit", { file_path: "a.ts", old_string: "one\ntwo\nthree", new_string: "one\nTWO\nthree" });
    expect(v.kind).toBe("diff");
    if (v.kind !== "diff") return;
    expect(v.rows.map((r) => `${r.kind}:${"text" in r ? r.text : ""}`)).toEqual(["ctx:one", "del:two", "add:TWO", "ctx:three"]);
    expect([v.adds, v.dels]).toEqual([1, 1]);
  });
  it("draws a written file as added lines", () => {
    const v = toolView("Write", { file_path: "b.ts", content: "x\ny\n" });
    expect(v.kind === "diff" && v.rows.map((r) => r.kind)).toEqual(["add", "add"]);
  });
  it("separates a multi-edit's replacements", () => {
    const v = toolView("MultiEdit", { file_path: "c.ts", edits: [{ old_string: "a", new_string: "b" }, { old_string: "c", new_string: "d" }] });
    expect(v.kind === "diff" && v.rows.map((r) => r.kind)).toEqual(["del", "add", "hunk", "del", "add"]);
  });
  it("reads a todo list as a checklist", () => {
    expect(toolView("TodoWrite", { todos: [{ content: "Write it", status: "completed" }] })).toEqual({ kind: "todos", todos: [{ text: "Write it", status: "completed" }] });
  });
  it("shows a command as the command", () => {
    expect(toolView("Bash", { command: "ls" })).toEqual({ kind: "command", command: "ls" });
  });
  it("falls back to the raw input for a shape it does not know", () => {
    expect(toolView("Edit", { TargetFile: "a.rs", ReplacementChunks: [] }).kind).toBe("raw");
    expect(toolView("Read", { file_path: "a" }).kind).toBe("raw");
    expect(toolView("Bash", undefined).kind).toBe("raw");
  });
});

describe("describe", () => {
  it("says what a tool did in plain words", () => {
    expect(said("Edit", "/repo/src/app.tsx", { file_path: "/repo/src/app.tsx" })).toEqual({ verb: "Edited", object: "app.tsx" });
    expect(said("Bash", "npm test", { command: "npm test" })).toEqual({ verb: "Ran", object: "npm test" });
    expect(said("TodoWrite", "3 todos", {})).toEqual({ verb: "Updated the plan", object: "" });
  });
  it("keeps an unknown tool's own name", () => {
    expect(said("mcp__x__y", "thing", {})).toEqual({ verb: "mcp__x__y", object: "thing" });
  });
});

describe("big writes", () => {
  it("draws only the rows that will be shown, and still counts every line", () => {
    const v = toolView("Write", { file_path: "lock.json", content: "x\n".repeat(30_000) });
    expect(v.kind === "diff" && v.rows.length).toBeLessThanOrEqual(401);
    expect(v.kind === "diff" && v.adds).toBe(30_000);
  });
});
