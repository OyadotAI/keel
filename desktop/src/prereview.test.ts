import { describe, expect, it, vi } from "vitest";

const answers: unknown[] = [];
vi.mock("./api", () => ({
  get: vi.fn(async () => answers.shift()),
  post: vi.fn(async () => undefined),
  url: () => "",
}));
// The Open step runs through `runs.ts`, whose stream lives in a Web Worker; nothing here opens one.
vi.mock("./runs", () => ({ start: vi.fn() }));

import { fixPrompt, navigable, refreshReview, reviewOf, sorted, type Finding, type Review } from "./prereview";

const ep = { port: 1, token: "t" };
const review = (outcome: Review["outcome"]): Review => ({ lane: "l", wt: "fix", sha: "abc1234def", base: "origin/main", started: 0, phase: outcome ? "done" : "reviewing", opening: { title: "", body: "", draft: false, base: "" }, by_agent: false, outcome });
const finding = (severity: string, title: string): Finding => ({ path: "src/a.ts", line: 3, severity, title, body: "" });

describe("the review before a pull request", () => {
  it("links only a path inside the checkout", () => {
    expect(navigable("src/a.ts")).toBe(true);
    for (const p of ["/etc/passwd", "../other/a.ts", "src/../../x", "C:\\a.ts", ""]) expect(navigable(p)).toBe(false);
  });

  it("sends the findings blocking first, and tells the agent not to open the pull request", () => {
    const text = fixPrompt(review(null), [finding("note", "n"), finding("blocking", "b")]);
    expect(text).toContain("`keel/fix` at abc1234 (against origin/main)");
    expect(text.indexOf("[blocking] b")).toBeLessThan(text.indexOf("[note] n"));
    expect(text).toContain("Do not open the pull request");
    expect(sorted([finding("warning", "w"), finding("blocking", "b")])[0].title).toBe("b");
  });

  it("says Keel restarted when a review it saw running is gone, and keeps saying it", async () => {
    answers.push({ review: null, head: "" });
    await refreshReview("quiet", ep);
    expect(reviewOf("quiet")?.restarted).toBe(false);

    answers.push({ review: review(null), head: "abc1234def" });
    await refreshReview("l", ep);
    expect(reviewOf("l")?.restarted).toBe(false);
    answers.push({ review: null, head: "abc1234def" });
    await refreshReview("l", ep);
    expect(reviewOf("l")?.restarted).toBe(true);
    answers.push({ review: null, head: "abc1234def" });
    await refreshReview("l", ep);
    expect(reviewOf("l")?.restarted).toBe(true);
  });

  it("does not call a finished review that was put away a restart", async () => {
    answers.push({ review: review({ kind: "none", cause: "refused", line: "x", took: 1 }), head: "" });
    await refreshReview("done", ep);
    answers.push({ review: null, head: "" });
    await refreshReview("done", ep);
    expect(reviewOf("done")?.restarted).toBe(false);
  });
});
