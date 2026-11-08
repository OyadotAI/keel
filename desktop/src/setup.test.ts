import { expect, it, vi } from "vitest";
import { get } from "./api";
import { chipState, readSetup, resultLines, type SetupRecord, type Warning } from "./setup";

vi.mock("./api", () => ({ get: vi.fn() }));

it("a new project's suggestions and findings are not warnings: only what stops a lane is", async () => {
  vi.mocked(get).mockImplementation(async (_ep, endpoint) => {
    if (endpoint === "/api/claude") return { installed: true, authenticated: true };
    if (endpoint === "/api/state") return { scan: { ignored: [], findings: ["agent/no-instructions", "agent/no-reviewers", "docs/no-readme", "security/untrusted-agent-config"].map((id) => ({ id, severity: "critical", title: id, detail: "" })) } };
    if (endpoint === "/api/license") return { steps: 90, cap: 100, licensee: null, email: "x", expires: null };
    return null;
  });
  const { warnings, readiness } = await readSetup({ port: 1234, token: "fixture" });
  expect(warnings).toEqual([]);
  expect(readiness).toEqual({ open: 4, critical: true });
});

it("a missing claude is a warning only for a project that runs claude", async () => {
  vi.mocked(get).mockImplementation(async (_ep, endpoint) => endpoint === "/api/claude" ? { installed: false, authenticated: false } : endpoint === "/api/state" ? { scan: { ignored: [], findings: [] } } : null);
  expect((await readSetup({ port: 1234, token: "fixture" })).warnings.map((w) => w.id)).toEqual(["claude/not-installed"]);
  expect((await readSetup({ port: 1234, token: "fixture" }, false)).warnings).toEqual([]);
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
  // A failure says so; untrusted ignores the record.
  expect(chipState(record({ phase: "failed", error: "no" }), [], true, false)).toMatchObject({ label: "Setup failed", icon: "octagon" });
  expect(chipState(record({ phase: "running" }), offer, false, false)).toMatchObject({ label: "Team update", color: "var(--dim)" });
  expect(chipState(undefined, [warning("claude/not-installed", "critical")], true, false).color).toBe("var(--del)");
  expect(chipState(record({ phase: "running" }), [warning("claude/not-installed", "critical")], true, false).color).toBe("var(--del)");
});

it("the result block names what landed and the commit each part is in", () => {
  const r = record({ phase: "done", team: { kind: "added", files: ["a", "b"], kept: ["c"], yours: [], commit: "abc" }, plugins: [{ name: "github", commit: "def" }, { name: "code-review", error: "offline" }] });
  expect(resultLines(r)).toEqual(["Team added · 2 files · commit abc · kept 1 you changed", "Plugin github installed · commit def"]);
});
it("only something that needs the person gets the alarm icon", () => {
  expect(chipState(record({ phase: "done", team: { kind: "updated", files: ["a"], kept: [], yours: [], commit: "abc" } }), [], true, false).icon).toBe("check-circle");
  expect(chipState(record({ phase: "running" }), [], true, false).icon).toBe("circle-dashed");
  expect(chipState(record({ phase: "waiting", waiting: { on: "you", paths: ["CLAUDE.md"] } }), [], true, false).icon).toBe("octagon");
  expect(chipState(undefined, [warning("team/update")], false, false).icon).toBe("list");
  expect(chipState(undefined, [{ ...warning("security/untrusted-agent-config"), severity: "critical" }], false, false).icon).toBe("octagon");
});
