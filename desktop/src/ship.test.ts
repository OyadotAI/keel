// Pins the one-click pull request's naming and when it makes a branch.
import { describe, expect, it, vi } from "vitest";
vi.mock("./api", () => ({ post: vi.fn() }));
vi.mock("./prereview", () => ({ openPullRequest: vi.fn() }));
import { branchName, needsBranch, shareKey, shareOffer, shipTitle } from "./ship";
import type { Lane } from "./store";

const conv = (prompt: string) => ({ turns: [{ prompt }] }) as unknown as Lane["conv"];

describe("ship", () => {
  it("names the work after the lane, else its first prompt, else a default", () => {
    expect(shipTitle({ title: "Dark mode toggle", conv: conv("x") })).toBe("Dark mode toggle");
    expect(shipTitle({ title: "Untitled session", conv: conv("make the header blue\nplease") })).toBe("make the header blue");
    expect(shipTitle({ title: "Untitled session", conv: { turns: [] } as unknown as Lane["conv"] })).toBe("Changes from Keel");
  });
  it("makes a safe branch name with a suffix", () => {
    expect(branchName("Fix: the Header's colour!", "ab12")).toBe("keel/fix-the-header-s-colour-ab12");
    expect(branchName("???", "ab12")).toBe("keel/changes-ab12");
  });
  it("branches only off the main line, and never in a lane with its own branch", () => {
    const git = (branch: string | null) => ({ is_repo: true, branch, changes: [] });
    expect(needsBranch({ git: git("main") })).toBe(true);
    expect(needsBranch({ git: git("feature/x") })).toBe(false);
    expect(needsBranch({ wt: "fix", git: git("keel/fix") })).toBe(false);
  });
  const work = (over: Partial<NonNullable<Lane["git"]>> = {}) => ({ is_repo: true, branch: "keel/x", changes: [], head: "aaa", ahead: 0, unsaved: 0, ...over }) as NonNullable<Lane["git"]>;
  it("offers nothing on a clean lane with nothing its base lacks", () => {
    expect(shareOffer(work())).toBeUndefined();
    expect(shareOffer(undefined)).toBeUndefined();
    // What Keel keeps out of commits is not work to share.
    expect(shareOffer(work({ changes: [{ path: ".mcp.json" }] as never, unsaved: 0 }))).toBeUndefined();
  });
  it("offers a pull request for unsaved changes or commits the base lacks", () => {
    expect(shareOffer(work({ unsaved: 2 }))).toBe("open");
    expect(shareOffer(work({ ahead: 3 }))).toBe("open");
  });
  it("offers nothing once the work is in a pull request, and an update when more lands", () => {
    const pr = { url: "https://github.com/o/r/pull/7", head: "aaa" };
    expect(shareOffer(work({ ahead: 3, pr }))).toBeUndefined();
    expect(shareOffer(work({ ahead: 4, head: "bbb", pr }))).toBe("update");
    expect(shareOffer(work({ unsaved: 1, pr }))).toBe("update");
  });
  it("stays closed until the head moves or the changes do", () => {
    const git = work({ ahead: 1 });
    expect(shareOffer(git, shareKey(git))).toBeUndefined();
    expect(shareOffer(work({ ahead: 2, head: "bbb" }), shareKey(git))).toBe("open");
    expect(shareOffer(work({ ahead: 1, unsaved: 1 }), shareKey(git))).toBe("open");
  });
});
