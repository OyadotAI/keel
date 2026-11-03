import { expect, it, vi } from "vitest";
import { get } from "./api";
import { chipState, readSetup, resultLines, type SetupRecord, type Warning } from "./setup";

vi.mock("./api", () => ({ get: vi.fn() }));

it("keeps distinct file paths for repeated findings so setup rows have stable identities", async () => {
  const paths = ["one/.claude/hooks/check.sh", "two/.claude/hooks/check.sh"];
  vi.mocked(get).mockImplementation(async (_ep, endpoint) => {
    if (endpoint === "/api/claude") return { installed: true, authenticated: true };
    if (endpoint === "/api/state") return { scan: { ignored: [], findings: paths.map((path) => ({ id: "security/untrusted-agent-config", severity: "critical", title: path, detail: "Review this hook", path, fix: { description: "Inspect it first" } })) } };
    if (endpoint === "/api/plugins") return { suggested: [] };
    return null;
  });
  const { warnings } = await readSetup({ port: 1234, token: "fixture" }, "/repo");
  expect(warnings.map((w) => w.path)).toEqual(paths);
  expect(new Set(warnings.map((w) => `${w.id}:${w.path}`)).size).toBe(2);
});

const record = (over: Partial<SetupRecord>): SetupRecord => ({ root: "/repo", phase: "idle", trigger: "open", plugins: [], at: 1, ...over });
const warning = (id: string, severity: Warning["severity"] = "medium"): Warning => ({ id, severity, title: id, detail: "", fix: "extensions" });

it("the chip reads the daemon's record in a trusted project and today's warnings otherwise", () => {
  const offer = [warning("team/update")];
  expect(chipState(record({ phase: "running" }), offer, true, false)).toMatchObject({ label: "Setting up…", color: "var(--dim)", aria: "Setup: running" });
  expect(chipState(record({ phase: "waiting", waiting: { on: "turn", paths: [] } }), offer, true, false).label).toBe("Setup · after this turn");
  expect(chipState(record({ phase: "waiting", waiting: { on: "merge", paths: [] } }), offer, true, false).label).toBe("Setup · after the merge");
  expect(chipState(record({ phase: "waiting", waiting: { on: "you", paths: ["CLAUDE.md"] } }), offer, true, false)).toMatchObject({ label: "1 waiting on you", color: "var(--warn)" });
  expect(chipState(record({ phase: "done", team: { kind: "updated", files: ["a"], kept: [], yours: [], commit: "abc" } }), [], true, false)).toMatchObject({ label: "Team updated", aria: "Setup: done, 1 commit" });
  expect(chipState(record({ phase: "done", team: { kind: "added", files: ["a"], kept: [], yours: [], commit: "abc" }, plugins: [{ name: "code-review", commit: "def" }] }), [], true, false)).toMatchObject({ label: "Set up", aria: "Setup: done, 2 commits" });
  // A failure falls back to the warning count in the warning colour; untrusted ignores the record.
  expect(chipState(record({ phase: "failed", error: "no" }), [warning("plugin/code-review")], true, false)).toMatchObject({ label: "1 to set up", color: "var(--warn)" });
  expect(chipState(record({ phase: "running" }), offer, false, false)).toMatchObject({ label: "Team update", color: "var(--dim)" });
  expect(chipState(undefined, [warning("claude/not-installed", "critical")], true, false).color).toBe("var(--del)");
  expect(chipState(record({ phase: "running" }), [warning("claude/not-installed", "critical")], true, false).color).toBe("var(--del)");
});

it("the result block names what landed and the commit each part is in", () => {
  const r = record({ phase: "done", team: { kind: "added", files: ["a", "b"], kept: ["c"], yours: [], commit: "abc" }, plugins: [{ name: "github", commit: "def" }, { name: "code-review", error: "offline" }] });
  expect(resultLines(r)).toEqual(["Team added · 2 files · commit abc · kept 1 you changed", "Plugin github installed · commit def"]);
});
