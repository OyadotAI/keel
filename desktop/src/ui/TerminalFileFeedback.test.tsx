// The actual terminal surface must expose the missing drop target without enabling shells/hidden lanes.
import { beforeEach, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { Lane } from "../store";
const fixture = vi.hoisted(() => ({ state: { active: "terminal", lanes: {} as Record<string, Lane> } }));
vi.mock("../store", () => ({ useStore: Object.assign((selector: (state: typeof fixture.state) => unknown) => selector(fixture.state), { setState: vi.fn() }) }));
vi.mock("../file-drag", () => ({ useFileDrag: (selector: (state: { drag?: unknown }) => unknown) => selector({}) }));
import { TerminalFileSurface } from "./TerminalFileFeedback";

beforeEach(() => {
  fixture.state.lanes.terminal = { id: "terminal", project: "/repo", interface: "terminal", agent: "claude", title: "Test", session: "session", isolated: false, known: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [] };
});
it("a visible provider terminal exposes its lane as an Explorer drop destination", () => {
  const html = renderToStaticMarkup(<TerminalFileSurface lane="terminal" agent hidden={false}><div className="terminal-host" /></TerminalFileSurface>);
  expect(html).toContain('data-reference-target="terminal"'); expect(html).toContain('class="terminal-host"');
});
it.each([{ agent: false, hidden: false }, { agent: true, hidden: true }])("shell/hidden surfaces are excluded: %j", (props) => {
  const html = renderToStaticMarkup(<TerminalFileSurface lane="terminal" {...props}><div /></TerminalFileSurface>);
  expect(html).not.toContain("data-reference-target");
});
it("rejections stay outside terminal scrollback and have a dismissible accessible alert", () => {
  fixture.state.lanes.terminal.referenceError = "Terminal isn’t ready.";
  const html = renderToStaticMarkup(<TerminalFileSurface lane="terminal" agent hidden={false}><div className="terminal-host" /></TerminalFileSurface>);
  expect(html).toContain('role="alert"'); expect(html).toContain("Dismiss file insertion error");
  expect(html.indexOf("Terminal isn’t ready.")).toBeLessThan(html.indexOf('class="terminal-host"'));
});
