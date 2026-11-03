// Handoff rendering exposes provenance and blocks continuation until checkout ownership is released.
import { expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { TaskView } from "../handoffs";
const fake = vi.hoisted(() => ({ view: undefined as TaskView | undefined, provider: "codex", source: "session", save: vi.fn(), proceed: vi.fn() }));
vi.mock("../store", () => ({ AGENT_NAME: { claude: "Claude", codex: "Codex", antigravity: "Antigravity" } }));
vi.mock("../handoffs/useHandoff", () => ({ useHandoff: () => ({ view: fake.view, busy: "", error: "", provider: fake.provider, lane: { session: fake.source }, setProvider: vi.fn(), save: fake.save, proceed: fake.proceed, stop: vi.fn(), refresh: vi.fn(), recover: vi.fn() }) }));
import { TaskHandoff } from "./TaskHandoff";
const source = { lane: "source", provider: "claude" as const, session: "session", checkout: "/scratch/repo" };
function fixture(): TaskView {
  return { task: { id: "task", revision: 4, owner: source, objective: "Finish recovery", notes: { decisions: "", completed: "", remaining: "", blockers: "", next_action: "Check current files" }, incomplete: true }, saved: { revision: 3, source, at: "2026-11-02T00:00:00Z", branch: "main", head: "old-head", incomplete: true, text: "Agent-reported: <script>unsafe</script>" }, events: [], truncated: false, current_branch: "main", current_head: "new-head", checkout_exists: true, ownership_error: "Source is still running", guidance: { status: "Available for the next Keel session", mechanism: "MCP instructions", hash: "hash", version: "v1" } };
}
it("shows exact escaped notes, revision, incomplete capture and changed-repository evidence without launching", () => {
  fake.view = fixture();
  const html = renderToStaticMarkup(<TaskHandoff lane="source" />);
  expect(html).toContain("revision 3"); expect(html).toContain("Incomplete capture"); expect(html).toContain("Repository state changed");
  expect(html).toContain("&lt;script&gt;"); expect(html).not.toContain("<script>");
  expect(html).toContain('class="primary" disabled=""'); expect(fake.proceed).not.toHaveBeenCalled();
});
it("keeps continuation unavailable when the original checkout is missing", () => {
  fake.view = { ...fixture(), checkout_exists: false, ownership_error: undefined };
  const html = renderToStaticMarkup(<TaskHandoff lane="source" />);
  expect(html).toContain("original checkout is unavailable"); expect(html).toContain('class="primary" disabled=""');
});
it("makes saved notes available independently of unsupported native context inspection", () => {
  fake.view = { ...fixture(), ownership_error: undefined };
  const html = renderToStaticMarkup(<TaskHandoff lane="source" />);
  expect(html).toContain("View handoff"); expect(html).toContain("Shared task changelog"); expect(html).toContain("Available for the next Keel session");
});
