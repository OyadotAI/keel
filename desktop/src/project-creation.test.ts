// Destination previews preserve platform path conventions without reserving or creating directories.
import { expect, it, vi } from "vitest";
vi.mock("./store", () => ({ useStore: {} }));
import { projectDestination } from "./project-creation";
it("preserves Windows home paths, spaces and Unicode", () => {
  expect(projectDestination("C:\\Users\\A Person\\Dev\\KeelWorkspace", "backend")).toBe("C:\\Users\\A Person\\Dev\\KeelWorkspace\\backend");
  expect(projectDestination("\\\\server\\日本語\\", "frontend")).toBe("\\\\server\\日本語\\frontend");
});
it("preserves macOS paths and avoids duplicate separators", () => {
  expect(projectDestination("/Users/a/Dev/KeelWorkspace/", " demo ")).toBe("/Users/a/Dev/KeelWorkspace/demo");
});
