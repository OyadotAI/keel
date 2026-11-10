// Pins the one-click pull request's naming and when it makes a branch.
import { describe, expect, it, vi } from "vitest";
vi.mock("./api", () => ({ post: vi.fn() }));
vi.mock("./prereview", () => ({ openPullRequest: vi.fn() }));
import { branchName, needsBranch, shipTitle } from "./ship";
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
});
