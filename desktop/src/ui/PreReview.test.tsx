/** The review dock: every state can be closed, and once a pull request exists nothing offers another. */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
const store = vi.hoisted(() => ({ state: { lanes: {} as Record<string, unknown> } }));
vi.mock("../store", () => ({ useStore: Object.assign((pick?: (s: typeof store.state) => unknown) => pick?.(store.state), { getState: () => store.state }) }));
vi.mock("./Approvals", () => ({ ApprovalStrip: () => null }));
vi.mock("../streams", () => ({ open: vi.fn() }));
vi.mock("../ide", () => ({ useIde: { getState: () => ({ navigate: () => undefined }) } }));
import { Ended, Finished, PullRequestLine, Running } from "./PreReview";
import { prLabel, prLink, prSaved, type Review } from "../prereview";

const ep = { port: 1, token: "t" };
const base = { lane: "l", ep, place: "dock" as const };
const review = (outcome?: Review["outcome"]): Review => ({
  id: "review-l", revision: 1, lane: "l", sha: "abc1234def", base: "origin/main", started: 0, phase: "reviewing",
  opening: { title: "", body: "", draft: false, base: "main" }, by_agent: false, outcome,
});
const url = "https://github.com/OyadotAI/oya-browser/pull/44";
const opened = (result: "done" | "failed") => ({ running: false, state: { lines: ["pushing", url], result } });
const buttons = (html: string) => [...html.matchAll(/<button[^>]*>([\s\S]*?)<\/button>/g)].map((m) => m[1].replace(/<[^>]+>/g, "").trim());

describe("the review dock", () => {
  it("says what a pull request carried: one commit named, several counted, else the branch", () => {
    expect(prSaved({ title: "Dark mode toggle", commits: 1, files: 3 }, "keel/dark")).toBe("Saved as “Dark mode toggle” · 3 files · on branch keel/dark");
    expect(prSaved({ title: "x", commits: 1, files: 1 }, "b")).toBe("Saved as “x” · 1 file · on branch b");
    expect(prSaved({ title: "last", commits: 4, files: 12 }, "b")).toBe("4 commits · 12 files · on branch b");
    expect(prSaved(undefined, "b")).toBe("On branch b");
    const html = renderToStaticMarkup(<PullRequestLine {...base} run={{ running: false, state: { lines: [url], result: "done", link: url, saved: { title: "Fix it", commits: 1, files: 2 }, target: { branch: "keel/fix" } } }} r={null} />);
    expect(html).toContain("Saved as “Fix it” · 2 files · on branch keel/fix");
  });
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

  it("uncommitted work or being on main is one step to take, with the save right there", () => {
    for (const line of ["There are uncommitted changes.", "You are on `main`. Make a branch for the change first."]) {
      store.state.lanes = { l: { git: { changes: [{}, {}] } } };
      const html = renderToStaticMarkup(<Ended {...base} r={review({ kind: "none", cause: "refused", line, took: 0 })} />);
      store.state.lanes = {};
      expect(html).toContain("Save your work first");
      expect(buttons(html)).toContain("Save and continue");
      for (const gone of ["Couldn’t review", "Technical details", "after 0s", "Review again"]) expect(html).not.toContain(gone);
    }
  });

  it("names a pull request as owner/repo#n, from gh's output whatever its result", () => {
    expect(prLabel(url)).toBe("OyadotAI/oya-browser#44");
    expect(prLink({ state: { lines: ["a pull request for branch x already exists:", url], result: "failed" } })).toBe(url);
    expect(prLink({ state: { lines: ["nothing"] } })).toBeUndefined();
  });
});

it("a recovered newer PR head is distinct from the requested or reviewed commit", () => {
  const run = { running: false, state: { lines: [], link: url, existing: true, actualHead: "fffffff111", target: { branch: "feature", head: "abc1234def", base: "main" } } };
  const html = renderToStaticMarkup(<PullRequestLine {...base} run={run} r={review()} />);
  expect(html).toContain("Pull request head fffffff");
  expect(html).toContain("Requested commit abc1234");
});
