// Branch discovery tests operate only on recorded metadata, without fetching or mutating Git.
import { expect, it } from "vitest";
import { branchResults, branchOwner, branchFailure, type Branches } from "./branches";
const data: Branches = { current: "main", local: [{ name: "main", current: true, ahead: 0, behind: 0, when: 10 }, { name: "Feature/日本語", current: false, ahead: 0, behind: 0, when: 20 }, { name: "alpha", current: false, ahead: 0, behind: 0, when: 20 }, { name: "unknown", current: false, ahead: 0, behind: 0 }], remote_details: [{ name: "origin/Feature", remote: "origin", when: 30, tracking: [] }], remotes: ["origin"], staged: 0, unstaged: 0 };
it("search is case insensitive and counts the pinned current branch without duplicating it", () => {
  expect(branchResults(data, "fEaTuRe", "all", "name").rows.map((b) => b.name)).toEqual(["Feature/日本語", "origin/Feature"]);
  expect(branchResults(data, "MAIN", "all", "name")).toEqual({ count: 1, rows: [] });
  expect(branchResults(data, "", "all", "name").count).toBe(5);
  expect(branchResults(data, "absent", "all", "name").count).toBe(0);
});
it("latest commit uses recorded timestamps, then name ties, with unknown dates last", () => {
  expect(branchResults(data, "", "all", "commit").rows.map((b) => b.name)).toEqual(["origin/Feature", "alpha", "Feature/日本語", "unknown"]);
  expect(branchResults(data, "", "remote", "name").rows).toHaveLength(1);
  expect(branchResults(data, "", "local", "name").count).toBe(4);
});
it("only verified worktree metadata can offer a known lane", () => {
  const branch = { ...data.local[1], kind: "local" as const, worktree: { path: "/repo/.keel/worktrees/one", keel: "one" } };
  const lanes = [{ id: "lane", project: "/repo", wt: "one", title: "Task" }];
  expect(branchOwner(branch, "/repo", lanes)?.id).toBe("lane");
  expect(branchOwner({ ...branch, worktree: undefined }, "/repo", lanes)).toBeUndefined();
  expect(branchOwner(branch, "/other", lanes)).toBeUndefined();
});
it("Git's known conflict receives guidance while unknown errors remain accurate", () => {
  expect(branchFailure(new Error("local changes would be overwritten by checkout"))).toContain("Commit or stash");
  expect(branchFailure(new Error("ref unavailable"))).toBe("ref unavailable");
});
