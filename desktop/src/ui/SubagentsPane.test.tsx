// Actual grid markup contains live child views only and never owns provider inputs.
import { beforeEach, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { Lane } from "../store";
interface FixtureState { active: string; lanes: Record<string, Lane>; projects: Record<string, { endpoint: {port:number;token:string} }> }
const fake = vi.hoisted(() => ({ state: {} as FixtureState }));
vi.mock("../store", () => ({ AGENT_NAME: { codex: "Codex" }, useStore: Object.assign((select: (state: FixtureState) => unknown) => select(fake.state), { getState: () => fake.state }) }));
vi.mock("../subagent-output", () => ({ useGridOutput: vi.fn() }));
vi.mock("../terminal-theme", () => ({ onTerminalTheme: () => () => {}, terminalDark: () => false }));
vi.mock("../zoom", () => ({ onTerminalFont: () => () => {}, terminalFont: () => 13 }));
import { SubagentsPane } from "./SubagentsPane";
import { childScope, patchChildOutput } from "../subagents";
const source: Lane = { id: "pane-test", project: "/scratch", session: "parent", interface: "terminal", agent: "codex", title: "Test", known: true, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [] };
const scope = childScope(source);
beforeEach(() => {
  const snapshot = { connected: true, scope, truncated: false, invocations: [{ id: "codex:child:run-one", child: "child", member: "pm", status: "working" as const, output: "waiting" as const }] };
  fake.state = { active: source.id, lanes: { [source.id]: { ...source, subagents: snapshot } }, projects: { [source.project]: { endpoint: { port: 1234, token: "synthetic" } } } };
});
const render = () => renderToStaticMarkup(<SubagentsPane lane={source.id} hidden={false}><div data-parent-host="same" /></SubagentsPane>);
it("gives every active child an independent read-only pane alongside the parent", () => {
  fake.state.lanes[source.id].subagents!.invocations.push({ id: "run-two", member: "pm", status: "working", output: "waiting" }, { id: "run-three", member: "qa", status: "waiting", output: "waiting" });
  const html = render();
  expect(html.match(/data-child-invocation=/g)).toHaveLength(3);
  expect(html).toContain("Codex · Main"); expect(html).toContain("Read only");
  expect(html.match(/terminal-pane-order">(\d)</g)).toEqual(['terminal-pane-order">1<', 'terminal-pane-order">2<', 'terminal-pane-order">3<', 'terminal-pane-order">4<']);
  expect(html.match(/terminal-pane-footer/g)).toHaveLength(4); expect(html).toContain("3 subagents working");
  expect(html).toContain('data-parent-host="same"'); expect(html).not.toContain('data-reference-target');
  expect(html).not.toContain("textarea"); expect(html).not.toContain("<input");
});
it("completed invocations do not leave a report or reduce the parent's space", () => {
  fake.state.lanes[source.id].subagents!.invocations[0].status = "done";
  const html = render();
  expect(html).not.toContain("data-child-invocation"); expect(html).not.toContain("split");
  expect(html).not.toContain("finished"); expect(html).toContain('data-parent-host="same"');
});
it("connection loss retains unconfirmed output without advertising working activity", () => {
  fake.state.lanes[source.id].subagents!.connected = false;
  patchChildOutput(scope, "codex:child:run-one", { detail: { items: [{ kind: "text", text: "<script>Useful result</script>" }, { kind: "tool", text: "Bash cat notes.md" }, { kind: "result", text: "first line\nsecond line\nthird line\n" }], availability: "available", truncated: true }, disconnected: true });
  const html = render();
  expect(html).toContain("status unconfirmed"); expect(html).toContain("Useful result");
  expect(html).toContain("<b>Bash</b>(cat notes.md)"); expect(html).toMatch(/<summary>.*first line.*\+2 lines.*<\/summary>/); expect(html).toContain("second line\nthird line");
  expect(html).toContain("Output disconnected"); expect(html).not.toContain("<script>");
  expect(html).not.toContain("is-working");
});
