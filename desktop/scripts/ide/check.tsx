// Synthetic checkout, real App/store/components. Never included in the shipped bundle.
import React from "react";
import { createRoot } from "react-dom/client";
import { useStore } from "../../src/store";
import { useIde } from "../../src/ide";
import { applyFrames } from "../../src/reduce";
import { setAppearance } from "../../src/appearance";
import { App } from "../../src/ui/App";
import "../../src/styles.css";
import "../../src/workspace.css";
import "../../src/ide.css";

const params = new URLSearchParams(location.search);
const wide = params.get("graph") === "wide";
const theme = params.get("theme");
if (theme === "light" || theme === "dark") setAppearance(theme);
if (params.has("sidebar")) localStorage.setItem("--coding-sidebar-w", params.get("sidebar")!);
if (wide) localStorage.setItem("keel.sidebar", "1");
function wideHistory() {
  const base = { author: "Keel", when: 1792100000, refs: [] as string[] };
  const sha = (n: number) => n.toString(16).padStart(40, "0");
  return [
    { ...base, sha: sha(0), parents: Array.from({ length: 23 }, (_, i) => sha(i + 1)), refs: ["HEAD -> refs/heads/main", "refs/remotes/origin/main"], subject: "Merge source control and navigation improvements" },
    ...Array.from({ length: 23 }, (_, i) => ({ ...base, sha: sha(i + 1), parents: [sha(24)], refs: [`refs/heads/feature/engineer-workspace-${i + 1}`], subject: `Improve workspace navigation ${i + 1}` })),
    ...Array.from({ length: 76 }, (_, i) => ({ ...base, sha: sha(i + 24), parents: i < 75 ? [sha(i + 25)] : [], subject: `Repository change ${i + 24}` })),
  ];
}
const project = "/workspace/keel";
const files: Record<string, string> = {
  "README.md": "# Keel\n\nAn agentic development environment.\n",
  "src/main.ts": 'import { createWorkspace } from "./workspace";\n\nconst workspace = createWorkspace();\nworkspace.followChanges(true);\n',
  "src/workspace.ts": Array.from({ length: 700 }, (_, i) => `export const setting${i} = ${i};`).join("\n"),
  "empty.ts": "",
  "large.lock": "large",
};
let fail = false;
let serial = 0;
let staged = false;
let committed = false;
const realFetch = window.fetch.bind(window);
function requestBody(init?: RequestInit) { return init?.body ? JSON.parse(String(init.body)) : {}; }
window.fetch = async (input, init) => {
  const target = new URL(String(input), location.href);
  if (!target.pathname.startsWith("/api/")) return realFetch(input, init);
  const path = target.searchParams.get("path") ?? "";
  const wt = target.searchParams.get("wt");
  if (target.pathname === "/api/file" && fail) { fail = false; return new Response("Synthetic read failure. Try again.", { status: 503 }); }
  let data: unknown = [];
  if (target.pathname === "/api/tree") data = [
    { name: "src", path: "src", dir: true, children: ["main.ts", "workspace.ts"].map((name) => ({ name, path: `src/${name}`, dir: false })) },
    ...["README.md", "empty.ts", "large.lock"].map((name) => ({ name, path: name, dir: false })),
  ];
  if (target.pathname === "/api/file") {
    if (path === "large.lock") data = { text: null, note: "This file exceeds the 1 MiB preview limit." };
    else data = { text: wt ? `// isolated checkout: ${wt}\n${files[path] ?? ""}` : files[path] ?? "", note: null };
    // A deliberately late response lets rapid selections verify stale-response handling.
    if (path === "empty.ts") await new Promise((resolve) => setTimeout(resolve, 1200));
  }
  const body = requestBody(init);
  if (target.pathname === "/api/git/act") staged = body.action === "stage";
  if (target.pathname === "/api/git/stage-all") staged = !!body.stage;
  if (target.pathname === "/api/git/commit-staged") committed = true;
  if (target.pathname === "/api/git/status") data = { is_repo: true, branch: wt ? "keel/feature" : "main", changes: committed ? [] : [{ path: "src/main.ts", status: staged ? "M " : " M", label: "Modified", staged, dir: false }] };
  if (target.pathname === "/api/git/branches") data = { current: "main", local: [{ name: "main", current: true, upstream: "origin/main", ahead: 1, behind: 0 }], remotes: ["origin"], staged: 0, unstaged: 1 };
  if (target.pathname === "/api/git/graph") data = wide ? wideHistory() : [
    { sha: "a".repeat(40), parents: ["b".repeat(40), "c".repeat(40)], refs: ["HEAD -> refs/heads/main", "refs/remotes/origin/main"], author: "Keel", when: 1792100000, subject: "Merge navigation improvements" },
    { sha: "c".repeat(40), parents: ["d".repeat(40)], refs: ["refs/heads/feature/navigation"], author: "Keel", when: 1792099900, subject: "Add file and symbol navigation" },
    { sha: "b".repeat(40), parents: ["d".repeat(40)], refs: [], author: "Keel", when: 1792099800, subject: "Improve source control experience" },
    { sha: "d".repeat(40), parents: ["e".repeat(40)], refs: ["tag: v0.3.0"], author: "Keel", when: 1792099700, subject: "Add integrated terminal sessions" },
    ...Array.from({ length: 32 }, (_, i) => ({ sha: i === 0 ? "e".repeat(40) : String(i).padStart(40, "0"), parents: i < 31 ? [String(i + 1).padStart(40, "0")] : [], refs: [], author: "Keel", when: 1792099600 - i * 60, subject: ["Keep changes visible across workspaces", "Fix keyboard focus in file picker", "Preserve terminal state on layout switch", "Show staged and working tree diffs"][i % 4] })),
  ];
  if (target.pathname === "/api/git/commit/diff") data = [{ path: "src/main.ts", untracked: false, hunks: [{ header: "@@ -1 +1 @@", lines: [{ kind: "del", old: 1, new: null, text: "workspace.followChanges(false);" }, { kind: "add", old: null, new: 1, text: "workspace.followChanges(true);" }] }] }];
  const canned = { "/api/git/log": [{ sha: "abcdef123456789", subject: "Add IDE workspace", when: 1792100000, files: 2, pushed: false }] };
  data = canned[target.pathname as keyof typeof canned] ?? data;
  if (target.pathname === "/api/git/diff") data = { path, untracked: false, hunks: [{ header: "@@ -1,2 +1,3 @@", lines: [
    { kind: "ctx", old: 1, new: 1, text: 'import { createWorkspace } from "./workspace";' },
    { kind: "del", old: 2, new: null, text: "workspace.followChanges(false);" },
    { kind: "add", old: null, new: 2, text: "workspace.followChanges(true);" },
  ] }] };
  return Response.json(data);
};
class DemoSocket extends EventTarget {
  static OPEN = 1;
  readyState = 0;
  binaryType = "arraybuffer";
  onopen?: () => void;
  onmessage?: (event: { data: ArrayBuffer | string }) => void;
  onclose?: (event: { code: number }) => void;
  constructor() { super(); setTimeout(() => { this.readyState = 1; this.onopen?.(); this.dispatchEvent(new Event("open")); this.output("Keel terminal fixture — simulated shell\r\n$ "); }, 80); }
  output(text: string) { this.onmessage?.({ data: new TextEncoder().encode(text).buffer }); }
  send(data: string | Uint8Array) { if (typeof data === "string") return; const text = new TextDecoder().decode(data); this.output(text === "\r" ? "\r\n$ " : text); }
  close() { this.readyState = 3; }
}
Object.assign(window, { WebSocket: DemoSocket });
Object.assign(window, {
  __TAURI_INTERNALS__: { metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } }, transformCallback: () => 1, unregisterCallback: () => {}, invoke: async () => 1 },
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
});
const conv = applyFrames({ turns: [] }, [
  { op: "open", turn: "demo", prompt: "Add an IDE layout so I can watch the code while we work." },
  { op: "text", turn: "demo", step: "answer", kind: "say", append: "The workspace keeps your files on the left, code in the center, and this conversation on the right.\n\nI’ll keep the selected checkout in view as changes arrive." },
]);
const lanes = Object.fromEntries(["main", "feature"].map((id) => [id, {
  id, project, title: id === "main" ? "Build the IDE workspace" : "Isolated feature", interface: "chat" as const, agent: "claude" as const,
  isolated: id === "feature", wt: id === "feature" ? "feature" : undefined,
  known: false, jobs: [], queued: [], conv, running: false, loaded: true, pending: [], draft: "Keep this draft when switching layouts.",
  git: { is_repo: true, branch: id === "main" ? "main" : "keel/feature", changes: [{ path: "src/main.ts", status: " M", label: "Modified", staged: false, dir: false }] },
}]));
useStore.setState({ lanes, order: [project], active: "main", opened: ["main"], projects: { [project]: { path: project, name: "Keel", lanes: Object.keys(lanes), endpoint: { port: 1420, token: "synthetic" } } }, trusted: { [project]: true }, load: () => {}, prepare: async (id) => ({ ep: { port: 1420, token: "synthetic" }, wt: id === "feature" ? "feature" : undefined }), ensure: async () => ({ port: 1420, token: "synthetic" }) });
useIde.getState().setLayout("ide");
if (wide) useIde.getState().setSidebar("git");
function edit() {
  const id = `edit-${++serial}`;
  const lane = useStore.getState().active!;
  const l = useStore.getState().lanes[lane];
  const path = serial % 2 ? "src/main.ts" : "src/workspace.ts";
  useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...l, reviewRevision: serial, conv: applyFrames(l.conv, [
    { op: "call", turn: "demo", id, tool: "Edit", subject: path, writes: [path] },
    { op: "result", turn: "demo", id, state: "ok" },
  ]) } } }));
}
const nextFrame = () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
async function verifyGraph() {
  const lanes = document.querySelector<HTMLElement>(".graph-lanes")!;
  const commits = document.querySelector<HTMLElement>(".graph-commits")!;
  const subject = document.querySelector<HTMLElement>(".graph-subject")!;
  const viewport = commits.getBoundingClientRect();
  const label = subject.getBoundingClientRect();
  if (label.width < 60 || label.left < viewport.left || label.right > viewport.right) throw new Error("Commit subject is clipped");
  const before = label.left;
  lanes.scrollLeft = 150;
  await nextFrame();
  if (lanes.scrollLeft < 1 || subject.getBoundingClientRect().left !== before) throw new Error("Graph pan displaced commit labels");
  commits.scrollTop = 250;
  await nextFrame();
  if (Math.abs(commits.scrollTop - lanes.scrollTop) > 1) throw new Error("Graph and rows lost vertical alignment");
  commits.scrollTop = commits.scrollHeight;
  await nextFrame();
  if (Math.abs(commits.scrollTop - lanes.scrollTop) > 1) throw new Error("Last commit is misaligned");
  lanes.scrollLeft = 0; commits.scrollTop = 0;
  await nextFrame();
  if (commits.scrollTop !== 0 || lanes.scrollTop !== 0) throw new Error("Scroll reset drifted");
  return "PASS: commit labels, horizontal graph pan, vertical alignment, and last row";
}
function Harness() {
  const [result, setResult] = React.useState("");
  return <><div style={{ height: 36, display: "flex", alignItems: "center", gap: 8, padding: "0 12px", fontSize: 11 }}>
    {wide && <><button onClick={() => void verifyGraph().then(setResult, (e) => setResult(`FAIL: ${String(e)}`))}>Check graph layout</button><output>{result}</output></>}<strong>Synthetic IDE QA</strong>{([['⌘P', 'KeyP', true, false, false], ['⌃G', 'KeyG', false, true, false], ['⌘F', 'KeyF', true, false, false], ['⇧⌘P', 'KeyP', true, false, true]] as const).map(([label, code, metaKey, ctrlKey, shiftKey]) => <button key={label} onClick={() => window.dispatchEvent(new KeyboardEvent('keydown', { code, metaKey, ctrlKey, shiftKey, bubbles: true }))}>{label}</button>)}<button onClick={edit}>Agent edit</button>
    <button onClick={() => useStore.setState({ active: useStore.getState().active === "main" ? "feature" : "main" })}>Switch lane</button>
    <button onClick={() => { fail = true; }}>Fail next read</button>
    <button onClick={() => useStore.setState((s) => ({ lanes: { ...s.lanes, [s.active!]: { ...s.lanes[s.active!], pending: [{ id: "q", tool: "AskUserQuestion", command: "", rules: [], input: { questions: [{ question: "Run the checks?", options: [{ label: "Unit tests" }, { label: "All checks" }] }] } }] } } }))}>Question</button>
  </div><div style={{ height: "calc(100% - 36px)" }}><App /></div></>;
}
createRoot(document.getElementById("root")!).render(<Harness />);
