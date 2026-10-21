/** The review dock: every state can be closed, and once a pull request exists nothing offers another. */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("../store", () => ({ useStore: Object.assign(() => undefined, { getState: () => ({ lanes: {} }) }) }));
vi.mock("./Approvals", () => ({ ApprovalStrip: () => null }));
vi.mock("../streams", () => ({ open: vi.fn() }));
vi.mock("../ide", () => ({ useIde: { getState: () => ({ navigate: () => undefined }) } }));
import { Ended, Finished, PullRequestLine, Running } from "./PreReview";
import { prLabel, prLink, type Review } from "../prereview";

const ep = { port: 1, token: "t" };
const base = { lane: "l", ep, place: "dock" as const };
const review = (outcome?: Review["outcome"]): Review => ({
  lane: "l", sha: "abc1234def", base: "origin/main", started: 0, phase: "reviewing",
  opening: { title: "", body: "", draft: false, base: "main" }, by_agent: false, outcome,
});
const url = "https://github.com/OyadotAI/oya-browser/pull/44";
const opened = (result: "done" | "failed") => ({ running: false, state: { lines: ["pushing", url], result } });
const buttons = (html: string) => [...html.matchAll(/<button[^>]*>([\s\S]*?)<\/button>/g)].map((m) => m[1].replace(/<[^>]+>/g, "").trim());

describe("the review dock", () => {
  it("once a pull request is open, offers no other — only its link", () => {
    const html = renderToStaticMarkup(<PullRequestLine {...base} run={opened("done")} r={review({ kind: "none", cause: "cancelled", line: "", took: 10 })} />);
    expect(html).toContain("Pull request opened");
    expect(html).toContain("OyadotAI/oya-browser#44");
    for (const offer of ["Open pull request", "Open without review", "Open anyway", "Review again"]) expect(buttons(html)).not.toContain(offer);
  });

  it("reads gh refusing an existing pull request as one already open", () => {
    const html = renderToStaticMarkup(<PullRequestLine {...base} run={opened("failed")} r={review()} />);
    expect(html).toContain("Pull request already open");
  });

  it("every state has a close control, and no text is put in an icon-sized ghost button", () => {
    const states = [
      <Running {...base} r={review()} head="" />,
      <Finished {...base} r={review()} head="" o={{ kind: "verdict", verdict: "ship", reason: "", summary: "", findings: [], report: "", took: 5 }} />,
      <Ended {...base} r={review({ kind: "none", cause: "cancelled", line: "", took: 10 })} />,
      <Ended {...base} r={null} />,
      <PullRequestLine {...base} run={opened("done")} r={review()} />,
      <PullRequestLine {...base} run={{ running: true, state: { lines: ["pushing"] } }} r={review()} />,
    ];
    for (const s of states) {
      const html = renderToStaticMarkup(s);
      expect(html).toMatch(/class="ghost review-close"[^>]*aria-label="(Dismiss review|Cancel review and close)"/);
      for (const m of html.matchAll(/<button class="ghost[^"]*"[^>]*>([\s\S]*?)<\/button>/g)) expect(m[1].replace(/<[^>]+>/g, "").trim()).toBe("");
    }
  });

  it("says a cancelled review opened nothing, and offers both ways on", () => {
    const html = renderToStaticMarkup(<Ended {...base} r={review({ kind: "none", cause: "cancelled", line: "", took: 10 })} />);
    expect(html).toContain("Review cancelled");
    expect(html).toContain("Nothing was opened.");
    expect(buttons(html)).toEqual(expect.arrayContaining(["Review again", "Open without review"]));
  });

  it("does not offer Open where Open would be refused the same way", () => {
    const html = renderToStaticMarkup(<Ended {...base} r={review({ kind: "none", cause: "refused", line: "You are on `main`.", took: 0 })} />);
    expect(buttons(html)).not.toContain("Open without review");
  });

  it("names a pull request as owner/repo#n, from gh's output whatever its result", () => {
    expect(prLabel(url)).toBe("OyadotAI/oya-browser#44");
    expect(prLink({ state: { lines: ["a pull request for branch x already exists:", url], result: "failed" } })).toBe(url);
    expect(prLink({ state: { lines: ["nothing"] } })).toBeUndefined();
  });
});
