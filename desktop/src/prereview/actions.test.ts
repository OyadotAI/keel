// Shared operation contracts pin failures, stale contexts, truthful delivery, and cancellation races.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Review, Seen } from "./model";
const api = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn(), open: vi.fn() }));
vi.mock("../api", async (original) => ({ ...await original<typeof import("../api")>(), get: api.get, post: api.post }));
vi.mock("../streams", () => ({ open: api.open }));

const ep = { port: 1234, token: "one" }, wt = "owned";
const review = (outcome: Review["outcome"] = { kind: "verdict", verdict: "do not ship", reason: "", summary: "", findings: [{ path: "a.ts", line: 1, severity: "blocking", title: "Finding", body: "" }], report: "", took: 1 }): Review => ({ id: "run-one", revision: 1, lane: "lane", wt, sha: "abc1234", base: "origin/main", phase: outcome ? "done" : "reviewing", started: 0, opening: { title: "Agent title", body: "Original description", draft: true, base: "main" }, by_agent: true, outcome });
let reading: Seen, saved: Map<string, string>;
let module: typeof import("../prereview");
async function seed(r: Review | null = review()) {
  reading = { review: r, head: "abc1234", target: { branch: "feature", base: "main", head: "abc1234" } };
  module.bindReview("lane", ep, wt); await module.refreshReview("lane", ep, wt, true);
}
beforeEach(async () => {
  vi.resetModules(); vi.clearAllMocks(); vi.useFakeTimers(); saved = new Map();
  vi.stubGlobal("localStorage", { getItem: (key: string) => saved.get(key) ?? null, setItem: (key: string, value: string) => saved.set(key, value) });
  api.get.mockImplementation(async () => structuredClone(reading));
  api.post.mockResolvedValue({ started: true, cancel_requested: true, finished: false, id: "run-one", dismissed: true });
  api.open.mockReturnValue({ close: vi.fn() });
  module = await import("../prereview");
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

it("read failure preserves useful results but revokes confirmation until reconnect succeeds", async () => {
  await seed(); api.get.mockRejectedValueOnce(new Error("offline"));
  expect(await module.refreshReview("lane", ep, wt, true)).toBe(false);
  expect(module.reviewOf("lane")).toMatchObject({ confirmed: false, review: { id: "run-one" }, error: { action: "read", details: "offline" } });
  expect(await module.refreshReview("lane", ep, wt, true)).toBe(true);
  expect(module.reviewOf("lane")?.error).toBeUndefined();
});

it("old endpoint responses and callbacks cannot rebind the current checkout", async () => {
  await seed(); let finish!: (value: Seen) => void;
  api.get.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
  const old = module.refreshReview("lane", ep, wt, true);
  const next = { port: 5678, token: "two" };
  module.bindReview("lane", next, "next");
  reading = { ...reading, review: { ...review(), id: "new-run" } };
  await module.refreshReview("lane", next, "next", true);
  finish({ ...reading, review: review() }); expect(await old).toBe(false);
  expect(await module.refreshReview("lane", ep, wt)).toBe(false);
  expect(module.reviewOf("lane")?.review?.id).toBe("new-run");
});

it("an older revision cannot overwrite a newer result or leave loading stuck", async () => {
  await seed({ ...review(), revision: 5 }); reading.review = review();
  expect(await module.refreshReview("lane", ep, wt, true)).toBe(false);
  expect(module.reviewOf("lane")).toMatchObject({ loading: false, confirmed: false, review: { revision: 5 } });
});

it("title and draft survive remount/reconnect without changing old opening metadata", async () => {
  await seed(); const original = module.reviewOf("lane")!.review!;
  module.updateReviewForm("lane", ep, wt, { title: "My new title", draft: false });
  module.bindReview("lane", { ...ep, token: "reconnected" }, wt);
  expect(module.reviewOf("lane")?.form).toMatchObject({ title: "My new title", draft: false });
  expect(original.opening.title).toBe("Agent title");
  vi.resetModules(); const next = await import("../prereview");
  expect(next.reviewForm("lane", wt)).toMatchObject({ title: "My new title", draft: false });
});

it("start is pending synchronously and duplicated clicks cannot start two reviews", async () => {
  await seed(null); module.updateReviewForm("lane", ep, wt, { title: "Preserved" });
  let finish!: (value: { started: boolean }) => void;
  api.post.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
  const first = module.startReview("lane", ep, wt);
  expect(module.reviewOf("lane")?.operation).toBe("start");
  expect(await module.startReview("lane", ep, wt)).toBe(false);
  expect(api.post).toHaveBeenCalledOnce(); expect(api.post.mock.calls[0][3]).toEqual({ wt });
  reading.review = { ...review(null), id: "new-run" }; finish({ started: true });
  expect(await first).toBe(true); expect(module.reviewOf("lane")?.operation).toBeUndefined();
  expect(module.reviewOf("lane")?.form.title).toBe("Preserved");
});

it("a failed findings delivery stays actionable and only accepted queueing is labelled queued", async () => {
  await seed();
  expect(await module.sendFindings("lane", ep, wt, review(), async () => false)).toBe(false);
  expect(module.reviewOf("lane")?.delivery).toBeUndefined();
  expect(module.reviewOf("lane")?.error?.action).toBe("fix");
  expect(await module.sendFindings("lane", ep, wt, review(), async () => "queued")).toBe(true);
  expect(module.reviewOf("lane")?.delivery).toEqual({ id: "run-one", state: "queued" });
});

it("findings pending in one surface cannot be sent again from another surface", async () => {
  await seed(); let finish!: (value: "sent") => void;
  const deliver = vi.fn(() => new Promise<"sent">((resolve) => { finish = resolve; }));
  const first = module.sendFindings("lane", ep, wt, review(), deliver);
  expect(await module.sendFindings("lane", ep, wt, review(), deliver)).toBe(false);
  expect(deliver).toHaveBeenCalledOnce(); finish("sent"); expect(await first).toBe(true);
});

it("dismissal waits for cancellation and never clears a live review after its request acknowledgement", async () => {
  await seed(review(null));
  api.post.mockImplementation(async (_ep, path) => {
    if (path.endsWith("dismiss")) { reading.review = null; return { dismissed: true }; }
    return { id: "run-one", cancel_requested: true, finished: false };
  });
  const closing = module.dismissReview("lane", ep, wt); await vi.advanceTimersByTimeAsync(0);
  expect(module.reviewOf("lane")?.review?.outcome).toBeNull();
  expect(api.post.mock.calls.some(([, path]) => path.endsWith("dismiss"))).toBe(false);
  reading.review = review({ kind: "none", cause: "cancelled", line: "", took: 1 });
  await vi.advanceTimersByTimeAsync(100); expect(await closing).toBe(true);
  expect(module.reviewOf("lane")).toMatchObject({ review: null, restarted: false });
  expect(module.reviewOf("lane")?.form.title).toBe("Agent title");
});

it("failed dismissal retains the review instead of clearing optimistically", async () => {
  await seed(); api.post.mockRejectedValueOnce(new Error("permission denied"));
  expect(await module.dismissReview("lane", ep, wt)).toBe(false);
  expect(module.reviewOf("lane")).toMatchObject({ review: { id: "run-one" }, error: { action: "dismiss" } });
});

it("cancellation has a bounded wait and leaves an unfinished review visible", async () => {
  await seed(review(null)); const stopping = module.stopReview("lane", ep, wt);
  await vi.advanceTimersByTimeAsync(10_000);
  expect(await stopping).toBe(false);
  expect(module.reviewOf("lane")).toMatchObject({ review: { outcome: null }, error: { action: "cancel" } });
  expect(module.reviewOf("lane")?.error?.details).toContain("still stopping");
});

it("opening waits for actual cancellation and keeps form/body and immutable target guards", async () => {
  await seed(review(null)); module.updateReviewForm("lane", ep, wt, { title: "Edited title", draft: false });
  const opening = module.openPullRequest(ep, wt, { title: "ignored old title", draft: true }, "lane");
  await vi.advanceTimersByTimeAsync(0); expect(api.open).not.toHaveBeenCalled();
  reading.review = review({ kind: "none", cause: "cancelled", line: "", took: 1 });
  await vi.advanceTimersByTimeAsync(100); expect(await opening).toBe(true);
  const request = new URL(api.open.mock.calls[0][0]);
  expect(request.searchParams.get("title")).toBe("Edited title");
  expect(request.searchParams.get("body")).toBe("Original description");
  expect(request.searchParams.get("head")).toBe("abc1234"); expect(request.searchParams.get("branch")).toBe("feature");
  expect(await module.openPullRequest(ep, wt, { title: "Again", draft: false }, "lane")).toBe(false);
  expect(api.open).toHaveBeenCalledOnce();
});

it("a fatal or truncated opening stream cannot be reported as success", async () => {
  const { prStep, prEnd } = await import("./opening");
  const failed = prStep(prStep({ lines: [] }, { event: "fatal", data: "offline" }), { event: "done", data: "0" });
  expect(prEnd(failed).result).toBe("failed"); expect(prEnd({ lines: [] }).result).toBe("failed");
});

it("confirmed dismissal retires a finished publication without discarding the form", async () => {
  await seed(); module.updateReviewForm("lane", ep, wt, { title: "Keep my title" });
  await module.openPullRequest(ep, wt, { title: "", draft: false }, "lane");
  const ended = api.open.mock.calls[0][3];
  ended("synthetic lost response");
  api.post.mockImplementation(async () => { reading.review = null; return { dismissed: true }; });
  expect(await module.dismissReview("lane", ep, wt)).toBe(true);
  const { prRun } = await import("./opening");
  expect(prRun(ep, wt)).toBeUndefined();
  expect(module.formPresentation(module.reviewOf("lane"), undefined).hasResult).toBe(false);
  expect(module.reviewOf("lane")?.form.title).toBe("Keep my title");
});

it("retry retains the captured failed target when the current checkout moves", async () => {
  await seed(); await module.openPullRequest(ep, wt, { title: "", draft: false }, "lane");
  api.open.mock.calls[0][3]("response lost");
  const { prRun } = await import("./opening");
  const captured = prRun(ep, wt)!.state.target;
  reading = { ...reading, head: "new-head", target: { branch: "feature", base: "new-base", head: "new-head" } };
  expect(await module.openPullRequest(ep, wt, { title: "", draft: false, ...captured }, "lane")).toBe(true);
  const request = new URL(api.open.mock.calls[1][0]);
  expect(request.searchParams.get("head")).toBe("abc1234");
  expect(request.searchParams.get("branch")).toBe("feature");
  expect(request.searchParams.get("base")).toBe("main");
});
