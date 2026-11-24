// Synthetic source transitions exercise the actual App, cards, focus and layout without agents.
import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "../../src/ui/App";
import { useStore, type Lane, type Pending } from "../../src/store";
import { applyFrames } from "../../src/reduce";
import { useIde } from "../../src/ide";
import { setAppearance } from "../../src/appearance";
import { MAC } from "../../src/keys";
import { attentionFixtureData } from "./data";
import { verifyRequestControls } from "./layout";
import "../../src/styles.css";
import "../../src/workspace.css";
import "../../src/ide.css";
import "../../src/attention.css";

const params = new URLSearchParams(location.search);
const theme = params.get("theme");
if (theme === "light" || theme === "dark") setAppearance(theme);
const endpoint = { port: location.port ? Number(location.port) : 1420, token: "synthetic-fixture" };
Object.assign(window, { __TAURI_INTERNALS__: { metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } }, transformCallback: () => 1, unregisterCallback: () => {}, invoke: async () => 1 }, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} } });

const fetchPage = window.fetch.bind(window);
window.fetch = async (input, init) => {
  const target = new URL(String(input), location.href);
  if (!target.pathname.startsWith("/api/")) return fetchPage(input, init);
  return Response.json(attentionFixtureData(target.pathname));
};
class FixtureSocket extends EventTarget {
  static OPEN = 1;
  readyState = 0;
  onopen?: () => void;
  onmessage?: (event: { data: ArrayBuffer }) => void;
  onclose?: (event: { code: number }) => void;
  constructor() { super(); setTimeout(() => { this.readyState = 1; this.onopen?.(); this.dispatchEvent(new Event("open")); this.onmessage?.({ data: new TextEncoder().encode("Synthetic agent terminal — no process is running.\r\nPermission requested. Type a reply here.\r\n").buffer }); }, 150); }
  send() { /* The fixture never sends terminal input to a daemon. */ }
  close() { this.readyState = 3; }
}
window.WebSocket = FixtureSocket as unknown as typeof WebSocket;

const question: Pending = { id: "q-request", lane: "question", session_id: "fixture", tool: "AskUserQuestion", command: "", rules: [], input: { questions: [{ question: "Which checkout behavior should we keep?", options: [{ label: "Keep existing behavior" }, { label: "Use the new flow" }] }] } };
function ended(status: string, check?: string) {
  const conv = applyFrames({ turns: [] }, [{ op: "open", turn: "fixture-turn", prompt: "Improve checkout behavior" }, { op: "text", turn: "fixture-turn", step: "reply", kind: "say", append: "Synthetic result. Inspect the project checks before sharing the change." }, { op: "close", turn: "fixture-turn", reason: status }]);
  conv.turns[0] = { ...conv.turns[0], ended: true, gate: check ? { status: check, command: "pnpm test", problems: check === "failed" ? [{ file: "src/checkout.ts", line: 42, message: "Expected a retried checkout to retain its idempotency key" }] : [] } : undefined };
  return conv;
}
function make(id: string, title: string, patch: Partial<Lane> = {}): Lane {
  return { id, project: "/fixture/shop", title, agent: "claude", interface: "chat", known: false, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [], ...patch };
}
function reset() {
  const lanes = [
    make("question", "Choose checkout behavior", { pending: [question], questionDrafts: { [question.id]: { request: question, values: { 0: { custom: "Keep this saved answer", selected: [], mode: "custom" } }, error: "Answer delivery was not confirmed. Retry in the question card." } } }),
    make("native", "Approve deployment command", { agent: "codex", interface: "terminal", attention: { id: "native-request", kind: "permission" } }),
    make("error", "Recover interrupted connection", { project: "/fixture/api", error: "The connection dropped. Open this lane to inspect recovery.", disconnected: true }),
    make("checks", "Fix checkout idempotency", { project: "/fixture/api", agent: "codex", interface: "terminal", conv: ended("done", "failed") }),
    make("update", "Review checked result", { conv: ended("done", "passed"), unseen: true }),
    make("stopped", "Stopped by the engineer", { project: "/fixture/api", conv: ended("stopped"), unseen: true }),
    make("unloaded", "Conversation not loaded", { project: "/fixture/api", loaded: false }),
    make("long", "Long lane name " + "with details ".repeat(8), { error: "UnbrokenLongError".repeat(80), unseen: true }),
  ];
  if (params.has("many")) for (let index = 0; index < 40; index++) lanes.push(make(`extra-${index}`, `Background task ${index + 1}`, { unseen: true, conv: ended("done") }));
  const order = ["/fixture/shop", "/fixture/api"];
  const projects = Object.fromEntries(order.map((path) => [path, { path, name: path.split("/").at(-1)!, lanes: lanes.filter((lane) => lane.project === path).map((lane) => lane.id), collapsed: true, endpoint }]));
  useStore.setState({ order, projects, lanes: Object.fromEntries(lanes.map((lane) => [lane.id, lane])), active: "update", opened: ["update"], settings: false, extensions: false, creating: false, trusted: Object.fromEntries(order.map((path) => [path, true])),
    ensure: async () => endpoint, load: () => {}, prepare: async () => ({ ep: endpoint }),
    select: (id) => useStore.setState((state) => ({ active: id, opened: [...new Set([...state.opened, id])], settings: false, creating: false, extensions: false, lanes: { ...state.lanes, [id]: { ...state.lanes[id], unseen: false } } })),
  });
}
reset();
useIde.getState().setLayout(params.get("layout") === "ide" ? "ide" : "chat");
function patch(id: string, patch: Partial<Lane>) { useStore.setState((state) => ({ lanes: { ...state.lanes, [id]: { ...state.lanes[id], ...patch } } })); }
function remove(id: string) { useStore.setState((state) => ({ lanes: Object.fromEntries(Object.entries(state.lanes).filter(([key]) => key !== id)), projects: Object.fromEntries(Object.entries(state.projects).map(([path, project]) => [path, { ...project, lanes: project.lanes.filter((lane) => lane !== id) }])) })); }
function Controls() {
  const [visible, setVisible] = React.useState(!params.has("bare"));
  const [layoutResult, setLayoutResult] = React.useState("");
  const [focus, setFocus] = React.useState("No focused control");
  React.useEffect(() => {
    const changed = () => {
      const element = document.activeElement as HTMLElement | null;
      const turns = element?.closest<HTMLElement>("[data-attention-turns]");
      const request = element?.closest<HTMLElement>("[data-attention-request]");
      setFocus(turns ? `Turns: ${turns.dataset.attentionTurns}` : request ? `Request: ${request.dataset.attentionRequest}` : element?.closest(".shell-dock .xterm") ? "Shell terminal input" : element?.closest(".xterm") ? "Agent terminal input" : element?.textContent?.slice(0, 45) || element?.tagName || "None");
    };
    document.addEventListener("focusin", changed);
    return () => document.removeEventListener("focusin", changed);
  }, []);
  if (!visible) return layoutResult ? <output style={{ position: "fixed", bottom: 0, right: 0, zIndex: 100, background: "var(--raised)", padding: 4 }}>{layoutResult}</output> : null;
  return <div style={{ position: "fixed", top: 0, right: 0, zIndex: 100, display: "flex", gap: 4, maxWidth: "100vw", flexWrap: "wrap", background: "var(--raised)", padding: 4, fontSize: 12 }}>
    <strong>Synthetic Attention QA</strong><output>Focus: {focus}</output><button onClick={reset}>Reset scenarios</button>
    <button onClick={() => setVisible(false)}>Hide QA controls</button>
    <button onClick={() => { setVisible(false); requestAnimationFrame(() => requestAnimationFrame(() => setLayoutResult(verifyRequestControls(document)))); }}>Check request controls</button>
    <button onClick={() => patch("question", { pending: [], questionDrafts: {} })}>Resolve question</button>
    <button onClick={() => patch("checks", { conv: ended("done", "passed") })}>Pass checks</button>
    <button onClick={() => remove("question")}>Remove question lane</button>
    <button onClick={() => useStore.setState({ lanes: {}, projects: {}, order: [], active: undefined, opened: [] })}>No projects</button>
    <button onClick={() => { const state = useStore.getState(); for (const [id, lane] of Object.entries(state.lanes)) patch(id, { pending: [], attention: undefined, error: undefined, questionDrafts: {}, unseen: false, loaded: true, conv: lane.conv.turns.length ? ended("done", "passed") : lane.conv }); }}>Clear source conditions</button>
    <button onClick={() => useIde.getState().setLayout(useIde.getState().layout === "ide" ? "chat" : "ide")}>Toggle layout</button>
    <button onClick={() => { document.querySelector<HTMLElement>("[data-chat-input]")?.focus(); window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyJ", metaKey: MAC, ctrlKey: !MAC, shiftKey: !MAC })); }}>Open shell from composer</button>
  </div>;
}
createRoot(document.getElementById("root")!).render(<><App /><Controls /></>);
Object.assign(window, { attentionFixture: { useStore, useIde, reset, patch, remove } });
