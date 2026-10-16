import { expect, it } from "vitest";
import { fileQuery, ideCommand } from "./ide-keys";
const event = (code: string, modifiers: Partial<KeyboardEvent> = {}) => ({ code, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, ...modifiers });
it("matches the macOS VS Code navigation keys without taking ordinary terminal input", () => {
  expect(ideCommand(event("KeyP", { metaKey: true }), true)).toBe("files");
  expect(ideCommand(event("KeyP", { metaKey: true, shiftKey: true }), true)).toBe("commands");
  expect(ideCommand(event("KeyG", { ctrlKey: true }), true)).toBe("line");
  expect(ideCommand(event("KeyG", { ctrlKey: true, shiftKey: true }), true)).toBe("git");
  expect(ideCommand(event("Backquote", { ctrlKey: true }), true)).toBe("terminal");
  expect(ideCommand(event("KeyE", { metaKey: true, shiftKey: true }), true)).toBe("explorer");
  expect(ideCommand(event("KeyF", { metaKey: true }), true)).toBe("find");
  expect(ideCommand(event("KeyC", { ctrlKey: true }), true)).toBeUndefined();
  expect(ideCommand(event("KeyP", { ctrlKey: true }), true)).toBeUndefined();
  expect(ideCommand(event("KeyP", { metaKey: true, altKey: true }), true)).toBeUndefined();
});
it("matches Windows/Linux primary shortcuts independently from Keel’s legacy terminal convention", () => {
  expect(ideCommand(event("KeyP", { ctrlKey: true }), false)).toBe("files");
  expect(ideCommand(event("KeyP", { ctrlKey: true, shiftKey: true }), false)).toBe("commands");
  expect(ideCommand(event("KeyB", { ctrlKey: true }), false)).toBe("sidebar");
  expect(ideCommand(event("KeyJ", { ctrlKey: true }), false)).toBe("terminal");
  expect(ideCommand(event("KeyW", { ctrlKey: true }), false)).toBe("closeFile");
  expect(ideCommand(event("KeyP", { metaKey: true }), false)).toBeUndefined();
});
it("parses path/line navigation without confusing colons in file names or allowing zero/negative lines", () => {
  expect(fileQuery("src/main.ts:42")).toEqual({ query: "src/main.ts", line: 42 });
  expect(fileQuery("src/main.ts:42:8")).toEqual({ query: "src/main.ts", line: 42 });
  expect(fileQuery("main.ts:999999").line).toBe(20_000);
  expect(fileQuery("main.ts:0")).toEqual({ query: "main.ts:0" });
  expect(fileQuery("main.ts:-2")).toEqual({ query: "main.ts:-2" });
});
