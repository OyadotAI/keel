// Caller-facing chat workflows preserve context ownership, draft intent, and response receipts.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Lane, Pending } from "./store";

const mocks = vi.hoisted(() => ({ post: vi.fn(), get: vi.fn(), open: vi.fn(), invoke: vi.fn() }));
vi.mock("./api", async (original) => ({ ...await original<typeof import("./api")>(), post: mocks.post, get: mocks.get }));
vi.mock("./streams", () => ({ open: mocks.open }));
vi.mock("./notify", () => ({ notify: vi.fn(), needsYou: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
const ep = { port: 12345, token: "fixture" };
const reference = { project: "/repo", path: "src/日本語 file.ts" };
const request: Pending = { id: "question-1", lane: "test", tool: "AskUserQuestion", command: "", rules: [], session_id: "session", input: { questions: [{ id: "q", question: "Which approach?", options: [{ label: "A" }, { label: "B" }] }] } };

async function fixture() {
  const { useStore } = await import("./store");
  const lane: Lane = { id: "test", project: "/repo", title: "Test", interface: "chat", agent: "claude", known: true, session: "old-session", isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [], draft: "Existing draft" };
  useStore.setState({ lanes: { test: lane }, projects: { "/repo": { path: "/repo", name: "Repo", lanes: ["test"], endpoint: ep } }, order: ["/repo"], active: "test", opened: [], prepare: async () => ({ ep }) });
  return useStore;
}

beforeEach(() => {
  vi.resetModules(); vi.clearAllMocks(); vi.useFakeTimers();
  const saved = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => saved.get(key) ?? null, setItem: (key: string, value: string) => saved.set(key, value) });
  mocks.open.mockReturnValue({ close: vi.fn() }); mocks.get.mockResolvedValue([]); mocks.invoke.mockResolvedValue(ep);
  mocks.post.mockResolvedValue({ ok: true, accepted: true, running: false });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

it.each(["claude", "codex", "antigravity"] as const)("fresh %s chat preserves settings without taking the original checkout or context", async (agent) => {
  const store = await fixture();
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, agent, running: true, isolated: true, wt: "original", mode: "acceptEdits", model: "chosen", references: [reference] } } }));
  const next = store.getState().newConversationFrom("test")!;
  expect(store.getState().active).toBe(next);
  expect(store.getState().lanes[next]).toMatchObject({ project: "/repo", agent, interface: "chat", model: "chosen", mode: "acceptEdits", isolated: true, running: false, known: false, conv: { turns: [] } });
  expect(store.getState().lanes[next].wt).toBeUndefined();
  expect(store.getState().lanes[next].session).not.toBe("old-session");
  expect(store.getState().lanes[next].draft).toBeUndefined();
  expect(store.getState().lanes[next].references).toBeUndefined();
  expect(store.getState().lanes.test).toMatchObject({ running: true, wt: "original", draft: "Existing draft", references: [reference] });
  expect(mocks.post.mock.calls.some(([, path]) => /stop|control|worktree/.test(path))).toBe(false);
});

it("a fresh terminal conversation retains its interface and shared workspace", async () => {
  const store = await fixture();
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal" } } }));
  const next = store.getState().newConversationFrom("test")!;
  expect(store.getState().lanes[next]).toMatchObject({ interface: "terminal", isolated: false });
});

it("validated references preserve the draft and deduplicate without submitting", async () => {
  const store = await fixture();
  expect(await store.getState().referenceFile("test", reference)).toBe(true);
  expect(await store.getState().referenceFile("test", reference)).toBe(true);
  expect(store.getState().lanes.test).toMatchObject({ draft: "Existing draft", references: [reference], referenceStatus: "File already referenced." });
  expect(mocks.post.mock.calls.every(([, path]) => path === "/api/references/validate")).toBe(true);
  store.getState().removeReference("test", reference);
  expect(store.getState().lanes.test.references).toEqual([]);
});

it("foreign and stale references never create an accepted chip", async () => {
  const store = await fixture();
  expect(await store.getState().referenceFile("test", { ...reference, wt: "other" })).toBe(false);
  expect(mocks.post).not.toHaveBeenCalled();
  mocks.post.mockRejectedValue(new Error("This file is no longer available."));
  expect(await store.getState().referenceFile("test", reference)).toBe(false);
  expect(store.getState().lanes.test.references).toBeUndefined();
  expect(store.getState().lanes.test.referenceError).toContain("no longer available");
});

it("validation cannot retarget a file after its lane changes checkout", async () => {
  const store = await fixture(); let finish!: (value: unknown) => void;
  mocks.post.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const adding = store.getState().referenceFile("test", reference);
  await Promise.resolve();
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, wt: "changed" } } }));
  finish({ ok: true }); expect(await adding).toBe(false);
  expect(store.getState().lanes.test.references).toBeUndefined();
});

it.each(["claude", "codex", "antigravity"] as const)("%s references survive queue, edit, reload and submission receipt", async (agent) => {
  const store = await fixture();
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, agent, running: true, references: [reference] } } }));
  await store.getState().send("test", "Existing draft", [], undefined, store.getState().lanes.test.references);
  const queued = store.getState().lanes.test.queued[0];
  expect(queued.references).toEqual([reference]);
  expect(store.getState().lanes.test.references).toEqual([]);
  store.getState().editQueued("test", queued.id);
  expect(store.getState().lanes.test.references).toEqual([reference]);
  vi.resetModules(); const { useStore: restored } = await import("./store");
  expect(restored.getState().lanes.test.references).toEqual([reference]);
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, running: false } } }));
  await store.getState().send("test", "Existing draft", [], undefined, store.getState().lanes.test.references);
  const sent = mocks.post.mock.calls.find(([, path]) => path === "/api/chat/send")![2];
  expect(sent.references).toEqual([reference]); expect(sent.provider).toBe(agent);
  expect(store.getState().lanes.test.references).toEqual([]);
});

it("a rejected stale reference leaves an editable draft rather than an unchangeable retry receipt", async () => {
  const store = await fixture(); const { HttpError } = await import("./api");
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, references: [reference] } } }));
  mocks.post.mockRejectedValue(new HttpError("file no longer exists", 400));
  expect(await store.getState().send("test", "Existing draft", [], undefined, store.getState().lanes.test.references)).toBe(false);
  expect(store.getState().lanes.test).toMatchObject({ draft: "Existing draft", references: [reference] });
  expect(store.getState().lanes.test.receipt).toBeUndefined();
  store.getState().removeReference("test", reference);
  mocks.post.mockResolvedValue({ accepted: true, running: false });
  expect(await store.getState().send("test", "Existing draft")).toBe(true);
});

it("retrying a lost receipt preserves replacement context that was never submitted", async () => {
  const store = await fixture();
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, references: [reference] } } }));
  mocks.post.mockRejectedValueOnce(new Error("response lost"));
  await store.getState().send("test", "Existing draft", [], undefined, store.getState().lanes.test.references);
  const receipt = store.getState().lanes.test.receipt!;
  const replacement = { ...reference, path: "new file.ts" };
  store.getState().removeReference("test", reference);
  await store.getState().referenceFile("test", replacement);
  await store.getState().send("test", receipt.prompt, receipt.attachments, receipt.system, receipt.references);
  const sent = mocks.post.mock.calls.filter(([, path]) => path === "/api/chat/send");
  expect(sent[1][2].references).toEqual([reference]);
  expect(store.getState().lanes.test.references).toEqual([replacement]);
});

it("a rejected queued reference is retained for editing without an automatic retry loop", async () => {
  const store = await fixture(); const { HttpError } = await import("./api");
  mocks.post.mockRejectedValue(new HttpError("file no longer exists", 400));
  expect(await store.getState().send("test", "Earlier queued message", [], undefined, [reference])).toBe(false);
  expect(store.getState().lanes.test.queued[0]).toMatchObject({ text: "Earlier queued message", references: [reference], failed: true });
  expect(store.getState().lanes.test.draft).toBe("Existing draft");
  expect(mocks.post).toHaveBeenCalledOnce();
});

it("question writing survives option picks, lane changes, reload and request withdrawal", async () => {
  const store = await fixture();
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, pending: [request] } } }));
  store.getState().writeQuestion("test", request, 0, "My own\nanswer");
  store.getState().pickQuestion("test", request, 0, "A");
  expect(store.getState().lanes.test.questionDrafts?.[request.id].values[0]).toMatchObject({ custom: "My own\nanswer", selected: ["A"], mode: "options" });
  store.getState().newConversationFrom("test");
  store.setState((s) => ({ lanes: { ...s.lanes, test: { ...s.lanes.test, pending: [] } } }));
  store.getState().draft("test", "Existing draft");
  vi.resetModules(); const { useStore: restored } = await import("./store");
  expect(restored.getState().lanes.test.questionDrafts?.[request.id].values[0].custom).toBe("My own\nanswer");
  store.getState().useQuestionAsMessage("test", request.id);
  expect(store.getState().lanes.test.draft).toContain("Existing draft\n\nWhich approach?\nA\nMy own\nanswer");
  expect(store.getState().lanes.test.pending).toEqual([]);
  expect(mocks.post).not.toHaveBeenCalled();
});

it.each([undefined, "claude", "codex"] as const)("%s question delivery failure preserves text and permits a live retry", async (provider) => {
  const store = await fixture(); const question = { ...request, provider };
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, pending: [question] } } }));
  store.getState().writeQuestion("test", question, 0, "My response");
  mocks.post.mockRejectedValueOnce(new Error("connection lost"));
  expect(await store.getState().answerQuestion("test", question)).toBe(false);
  expect(store.getState().lanes.test.questionDrafts?.[question.id]).toMatchObject({ sending: false, values: { 0: { custom: "My response" } } });
  expect(store.getState().lanes.test.pending).toHaveLength(1);
  expect(await store.getState().answerQuestion("test", question)).toBe(true);
  expect(store.getState().lanes.test.questionDrafts?.[question.id]).toBeUndefined();
  expect(store.getState().lanes.test.pending).toHaveLength(0);
});

it("expired requests cannot falsely accept or erase a composed answer", async () => {
  const store = await fixture();
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, pending: [request] } } }));
  store.getState().writeQuestion("test", request, 0, "Too late");
  mocks.post.mockResolvedValue({ ok: false, closed: true, reason: "no longer waiting" });
  expect(await store.getState().answerQuestion("test", request)).toBe(false);
  expect(store.getState().lanes.test.pending).toEqual([]);
  expect(store.getState().lanes.test.questionDrafts?.[request.id]).toMatchObject({ closed: true, values: { 0: { custom: "Too late" } } });
});

it("repeated questions and duplicate submit clicks have independent request ownership", async () => {
  const store = await fixture(); let finish!: (value: unknown) => void;
  const second = { ...request, id: "question-2" };
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, pending: [request, second] } } }));
  store.getState().writeQuestion("test", request, 0, "First");
  store.getState().writeQuestion("test", second, 0, "Second");
  mocks.post.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const submitting = store.getState().answerQuestion("test", request);
  expect(await store.getState().answerQuestion("test", request)).toBe(false);
  expect(mocks.post).toHaveBeenCalledOnce();
  finish({ ok: true }); await submitting;
  expect(store.getState().lanes.test.questionDrafts?.[second.id].values[0].custom).toBe("Second");
  expect(store.getState().lanes.test.pending).toEqual([second]);
});

it.each(["claude", "codex", "antigravity"] as const)("explicit %s continuation retains the checkout and retries its durable receipt", async (provider) => {
  const store = await fixture();
  store.setState((s) => ({ lanes: { test: { ...s.lanes.test, isolated: true, wt: "existing-tree", mode: "acceptEdits" } } }));
  const result = { id: "stable-receipt", revision: 7, source: { lane: "test", provider: "claude" as const, session: "old-session", checkout: "/repo/.keel/worktrees/existing-tree", wt: "existing-tree" }, destination: { lane: "continued", provider, session: "pending-receipt", checkout: "/repo/.keel/worktrees/existing-tree", wt: "existing-tree" }, prompt: "Inspected revision seven" };
  const destination = store.getState().adoptContinuation("test", result);
  expect(store.getState().lanes[destination]).toMatchObject({ agent: provider, interface: "chat", wt: "existing-tree", known: false, receipt: { id: "stable-receipt", prompt: "Inspected revision seven" } });
  expect(store.getState().lanes.test).toMatchObject({ session: "old-session", wt: "existing-tree", draft: "Existing draft" });
  expect(store.getState().adoptContinuation("test", result)).toBe(destination);
  expect(store.getState().projects["/repo"].lanes.filter((id) => id === destination)).toHaveLength(1);
  await store.getState().send(destination, result.prompt);
  expect(mocks.post.mock.calls.find(([, path]) => path === "/api/chat/send")?.[2]).toMatchObject({ id: "stable-receipt", provider });
  expect(mocks.post.mock.calls.some(([, path]) => path === "/api/worktree/create")).toBe(false);
});
