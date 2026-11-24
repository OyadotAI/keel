// The IDE mounts branch discovery even when its Git sidebar is hidden.
import { expect, it } from "vitest";
import { branchResults, type Branches } from "../../src/branches";
import { attentionFixtureData } from "./data";

it("keeps the real IDE branch consumers usable in the synthetic Attention workspace", () => {
  const branches = attentionFixtureData("/api/git/branches") as Branches;
  expect(branches.local.find((branch) => branch.current)?.name).toBe("keel/attention-qa");
  expect(branchResults(branches, "", "all", "name")).toEqual({ rows: [], count: 1 });
});
