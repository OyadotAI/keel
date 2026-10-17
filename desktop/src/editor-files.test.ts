/** Editor buffers keep human edits across navigation, retries and concurrent disk changes. */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { post } from "./api";
import { editorFileKey, persistEditorDrafts, saveEditorFile, useEditorFiles } from "./editor-files";
vi.mock("./api", () => ({ post: vi.fn() }));
const ep = { port: 1234, token: "fixture" };
const key = editorFileKey("/repo", undefined, "code.ts");
beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("localStorage", { getItem: vi.fn(), setItem: vi.fn() });
  useEditorFiles.setState({ files: {}, storageError: undefined });
  vi.mocked(post).mockReset();
});
afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals(); });
it("keeps drafts per checkout and path, including shared lanes and isolated worktrees", () => {
  const other = editorFileKey("/repo", "feature", "code.ts");
  const store = useEditorFiles.getState();
  store.receive(key, "original"); store.edit(key, "draft"); store.receive(other, "feature");
  store.receive(key, "external change");
  expect(useEditorFiles.getState().files[key]).toMatchObject({ base: "original", text: "draft", error: expect.stringContaining("changed on disk") });
  expect(useEditorFiles.getState().files[other].text).toBe("feature");
});
it("updates clean buffers from disk without making them dirty", () => {
  useEditorFiles.getState().receive(key, "old");
  useEditorFiles.getState().receive(key, "new");
  expect(useEditorFiles.getState().files[key]).toMatchObject({ base: "new", text: "new" });
});
it("saves the exact original and worktree while retaining edits made during the save", async () => {
  useEditorFiles.getState().receive(key, "original"); useEditorFiles.getState().edit(key, "first edit");
  let done!: () => void;
  vi.mocked(post).mockImplementation(() => new Promise<void>((resolve) => { done = resolve; }));
  const saving = saveEditorFile(key, ep, "code.ts", "feature");
  expect(await saveEditorFile(key, ep, "code.ts", "feature")).toBe(false);
  useEditorFiles.getState().edit(key, "second edit"); done();
  expect(await saving).toBe(true);
  expect(post).toHaveBeenCalledWith(ep, "/api/file", { path: "code.ts", text: "first edit", original: "original" }, { wt: "feature" });
  expect(useEditorFiles.getState().files[key]).toMatchObject({ base: "first edit", text: "second edit", saving: false });
});
it("retains edits after a failed save and supports explicit reload", async () => {
  useEditorFiles.getState().receive(key, "original"); useEditorFiles.getState().edit(key, "draft");
  vi.mocked(post).mockRejectedValue(Error("changed on disk"));
  expect(await saveEditorFile(key, ep, "code.ts")).toBe(false);
  expect(useEditorFiles.getState().files[key]).toMatchObject({ text: "draft", base: "original", error: "changed on disk" });
  useEditorFiles.getState().discard(key, "disk");
  expect(useEditorFiles.getState().files[key]).toEqual({ text: "disk", base: "disk" });
});
it("persists only unsaved buffers and reports actual storage failures", () => {
  useEditorFiles.getState().receive(key, "base"); useEditorFiles.getState().edit(key, "draft");
  useEditorFiles.getState().receive("clean", "clean");
  persistEditorDrafts();
  expect(JSON.parse(vi.mocked(localStorage.setItem).mock.calls.at(-1)![1])).toEqual({ [key]: { base: "base", text: "draft" } });
  vi.mocked(localStorage.setItem).mockImplementation(() => { throw Error("full"); });
  persistEditorDrafts();
  expect(useEditorFiles.getState().storageError).toContain("cannot be stored");
  expect(useEditorFiles.getState().files[key].text).toBe("draft");
});
