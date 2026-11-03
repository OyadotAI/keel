// Rendered pane semantics keep child observation read-only and independent of terminal input.
import { beforeEach, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { Lane } from "../store";
interface FixtureState { active: string; lanes: Record<string, Lane>; projects: Record<string, { endpoint: {port:number;token:string} }> }
const fake = vi.hoisted(() => ({ state: {} as FixtureState }));
vi.mock("../store", () => ({ useStore: Object.assign((select: (state: FixtureState) => unknown) => select(fake.state), { getState: () => fake.state }) }));
vi.mock("../subagent-output", () => ({ useChildOutput: vi.fn() }));
import { SubagentsPane } from "./SubagentsPane";
import { childScope, patchChildOutput, patchPane, reconcilePane } from "../subagents";
const source: Lane = { id: "pane-test", project: "/scratch", session: "parent", interface: "terminal", agent: "codex", title: "Test", known: true, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [] };
const scope = childScope(source);
beforeEach(() => {
  const snapshot = { connected: true, scope, truncated: false, invocations: [{ id: "codex:child:run-one", child: "child", member: "pm", status: "working" as const, output: "waiting" as const }] };
  fake.state = { active: source.id, lanes: { [source.id]: { ...source, subagents: snapshot } }, projects: { [source.project]: { endpoint: { port: 1234, token: "synthetic" } } } };
  reconcilePane(scope, snapshot); patchPane(scope, { open: true });
});
it("shows invocation identity and explicit status without an interactive terminal or file-drop destination", () => {
  const html = renderToStaticMarkup(<SubagentsPane lane={source.id} hidden={false} />);
  expect(html).toContain('aria-label="Subagents"'); expect(html).toContain("Working"); expect(html).toContain("run-one");
  expect(html).toContain("Read only"); expect(html).toContain('data-reference-target=""');
  expect(html).not.toContain("textarea"); expect(html).not.toContain("<input");
});
it("connection loss revokes current member animation while retaining readable progress", () => {
  fake.state.lanes[source.id].subagents!.connected = false;
  patchChildOutput(scope, "codex:child:run-one", { detail: { items: [{ kind: "text", text: "Useful result" }], availability: "available", truncated: true }, disconnected: true });
  const html = renderToStaticMarkup(<SubagentsPane lane={source.id} hidden={false} />);
  expect(html).toContain("Status unknown"); expect(html).toContain("Useful result"); expect(html).toContain("Output disconnected"); expect(html).toContain("Showing recent output");
  expect(html).not.toContain("is-working");
});
it("collapse removes only the output surface and keeps a discoverable activity header", () => {
  patchPane(scope, { open: false });
  const html = renderToStaticMarkup(<SubagentsPane lane={source.id} hidden={false} />);
  expect(html).toContain('aria-expanded="false"'); expect(html).toContain("1 working"); expect(html).not.toContain("subagent-output-text");
});
it("shows custom and unnamed native children without requiring an installed team profile", () => {
  const children = fake.state.lanes[source.id].subagents!;
  children.invocations.push({ id: "custom-run", child: "custom-child", member: "repository-explorer", status: "working", output: "waiting" });
  children.invocations.push({ id: "unnamed-run", child: "unnamed-child", member: null, status: "working", output: "waiting" });
  reconcilePane(scope, children);
  const html = renderToStaticMarkup(<SubagentsPane lane={source.id} hidden={false} />);
  expect(html).toContain("repository-explorer"); expect(html).toContain("unnamed"); expect(html).toContain("3 working");
});
