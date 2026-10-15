import { beforeEach, expect, it, vi } from "vitest";
import { install, installSetup } from "./setup-install";
import { post } from "./api";
import type { Warning } from "./setup";
import { open } from "./streams";
vi.mock("./streams", () => ({ open: vi.fn() }));
vi.mock("./api", async (original) => ({ ...await original<typeof import("./api")>(), post: vi.fn() }));
beforeEach(() => vi.resetAllMocks());
const ep = { port: 1234, token: "secret" };
it("installs at project scope and waits for confirmed success", async () => {
  const done = install(ep, "test", "official");
  const [url, token, events, end] = vi.mocked(open).mock.calls[0];
  expect(url).toContain("name=test&marketplace=official&commit=true");
  expect(token).toBe("secret");
  events([{ event: "done", data: "0" }]); end();
  await expect(done).resolves.toBeUndefined();
});
it("keeps the installer failure instead of discarding its reason", async () => {
  const done = install(ep, "test", "official");
  const [, , events, end] = vi.mocked(open).mock.calls[0];
  events([{ event: "line", data: "Project is busy" }, { event: "done", data: "1" }]); end();
  await expect(done).resolves.toBe("Project is busy");
});
it("reports HTTP errors and incomplete streams", async () => {
  const failed = install(ep, "test", "official");
  vi.mocked(open).mock.calls[0][3]("Unauthorized");
  await expect(failed).resolves.toBe("Unauthorized");
  const incomplete = install(ep, "test", "official");
  vi.mocked(open).mock.calls[1][3]();
  await expect(incomplete).resolves.toMatch(/without confirming/);
});

const teamWarning: Warning = { id: "agent/no-reviewers", title: "No team", detail: "Missing agents", fix: "readiness", severity: "high" };
const pluginWarning: Warning = { id: "plugin/reviewer", title: "Missing plugin", detail: "Missing reviewer", fix: "extensions", severity: "medium", marketplace: "official" };

it("installs the team and refreshes members before waiting on a plugin download", async () => {
  vi.mocked(post).mockResolvedValue({ written: [".claude/agents/reviewer.md"], skipped: [] });
  const progress = vi.fn();
  const changed = vi.fn();
  let settled = false;
  const done = installSetup(ep, [pluginWarning, teamWarning], progress, changed).then((result) => { settled = true; return result; });
  await vi.waitFor(() => expect(open).toHaveBeenCalledOnce());
  expect(post).toHaveBeenCalledWith(ep, "/api/adopt", undefined, { commit: true });
  expect(changed).toHaveBeenCalledOnce();
  expect(changed.mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(open).mock.invocationCallOrder[0]);
  expect(settled).toBe(false);
  const [, , events, end] = vi.mocked(open).mock.calls[0];
  events([{ event: "line", data: "Downloading reviewer…" }]);
  expect(progress).toHaveBeenLastCalledWith(expect.stringContaining("Downloading reviewer…"));
  events([{ event: "done", data: "0" }]); end();
  await expect(done).resolves.toMatchObject({ installed: 1, total: 1, errors: [] });
});

it("continues after failures and reports each installer error instead of reporting success", async () => {
  vi.mocked(post).mockRejectedValue(new Error("Project is busy"));
  const changed = vi.fn();
  const second = { ...pluginWarning, id: "plugin/security" };
  const done = installSetup(ep, [teamWarning, pluginWarning, second], vi.fn(), changed);
  await vi.waitFor(() => expect(open).toHaveBeenCalledOnce());
  vi.mocked(open).mock.calls[0][2]([{ event: "fatal", data: "Marketplace unavailable" }, { event: "done", data: "1" }]);
  vi.mocked(open).mock.calls[0][3]();
  await vi.waitFor(() => expect(open).toHaveBeenCalledTimes(2));
  vi.mocked(open).mock.calls[1][2]([{ event: "done", data: "0" }]);
  vi.mocked(open).mock.calls[1][3]();
  const result = await done;
  expect(result).toMatchObject({ installed: 1, total: 2 });
  expect(result.errors).toEqual(["Agent team: Project is busy", "reviewer: Marketplace unavailable"]);
  expect(changed).not.toHaveBeenCalled();
});

it("reports team files that could not be written even when the adopt request succeeds", async () => {
  vi.mocked(post).mockResolvedValue({ written: [], skipped: [".claude/agents/qa.md (Permission denied)", "CLAUDE.md (already says it)", "docs/PRODUCTION.md (already there)"] });
  const result = await installSetup(ep, [teamWarning], vi.fn(), vi.fn());
  expect(result.errors).toEqual(["Agent team: .claude/agents/qa.md (Permission denied)"]);
  expect(open).not.toHaveBeenCalled();
});
