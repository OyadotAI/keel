import { expect, it, vi } from "vitest";
import { get } from "./api";
import { readSetup } from "./setup";

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
