import { expect, it } from "vitest";
import { commitGraph, type GraphCommit, type GraphRow } from "./commit-graph";

const commit = (sha: string, ...parents: string[]): GraphCommit => ({ sha, parents, refs: [], author: "Engineer", when: 1, subject: sha });
function continuous(rows: GraphRow[]) {
  for (let i = 1; i < rows.length; i++) {
    const bottom = rows[i - 1].edges.filter((e) => e.half !== "top").map((e) => `${e.to}:${e.color}`);
    const top = rows[i].edges.filter((e) => e.half !== "bottom").map((e) => `${e.from}:${e.color}`);
    expect([...new Set(top)].sort()).toEqual([...new Set(bottom)].sort());
  }
}

it("keeps a linear history in one column and terminates at the root", () => {
  const rows = commitGraph([commit("a", "b"), commit("b", "c"), commit("c")]);
  expect(rows.map((r) => r.column)).toEqual([0, 0, 0]);
  expect(rows.at(-1)?.edges.every((e) => e.half === "top")).toBe(true);
  continuous(rows);
});

it("splits real merge parents into distinct colored lanes and rejoins their common ancestor", () => {
  const rows = commitGraph([commit("merge", "main", "feature"), commit("feature", "base"), commit("main", "base"), commit("base")]);
  expect(rows[0].edges.filter((e) => e.half === "bottom").map((e) => e.to)).toEqual([0, 1]);
  expect(rows[1].color).not.toBe(rows[2].color);
  expect(rows[3].width).toBe(1);
  continuous(rows);
});

it("preserves octopus merges, disconnected roots and continuation beyond the history limit", () => {
  const rows = commitGraph([commit("merge", "one", "two", "three"), commit("two", "base"), commit("one", "base"), commit("three", "base"), commit("base", "older"), commit("other")]);
  expect(rows[0].edges).toHaveLength(3);
  expect(rows[4].edges.some((e) => e.half === "bottom")).toBe(true);
  expect(rows[5].column).toBe(1);
  continuous(rows);
  expect(commitGraph([])).toEqual([]);
});
