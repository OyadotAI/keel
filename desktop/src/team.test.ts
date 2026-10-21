import { describe, expect, it } from "vitest";
import { absent, agentsIn, forAgents, offered, updatedSentence, type TeamPlan } from "./team";

const plan = (over: Partial<TeamPlan> = {}): TeamPlan => ({
  add: [], replace: [], kept: [], not_restored: [], present: true,
  agents: { claude: true, antigravity: true, codex: false }, ...over,
});

describe("team", () => {
  it("offers an update only to a project that has the team and something to write", () => {
    expect(offered(plan({ replace: ["a"] }))).toBe(true);
    expect(offered(plan({ replace: ["a"], present: false }))).toBe(false);
    expect(offered(plan({ kept: ["a"] }))).toBe(false);
  });

  it("names the agents it is for and the ones that are not on this machine", () => {
    expect(forAgents(plan())).toBe("Claude Code and Antigravity");
    expect(absent(plan())).toBe("Codex isn't on this machine.");
    expect(forAgents(undefined)).toBe("Claude Code, Antigravity and Codex");
    expect(agentsIn([".claude/agents/pm.md", ".codex/agents/pm.toml"])).toBe("Claude Code and Codex");
  });

  it("says what an update did, and what it left alone", () => {
    const r = { replaced: ["a"], added: ["b", "c"], kept: ["d"], not_restored: [], uncommitted: [] };
    expect(updatedSentence({ ...r, committed: "abc1234" })).toBe("Team updated in commit abc1234: 1 replaced, 2 added. Kept 1 file you changed.");
    expect(updatedSentence({ ...r, replaced: [], added: [] })).toBe("The team is up to date. Kept 1 file you changed.");
    expect(updatedSentence({ ...r, committed: "abc1234", lane: "team-update" })).toBe("A session was editing the project, so the update went to its own lane, keel/team-update: 1 replaced, 2 added. Kept 1 file you changed. Finish that lane to merge it.");
    expect(updatedSentence({ ...r, kept: [], note: "commits are off" })).toBe("Team updated: 1 replaced, 2 added. Not committed: commits are off.");
  });
});
