// The lane's Browser tab, from the page's side. The daemon says `browser` with an op for a lane;
// this runs it against that lane's native webview (`browser_act`) and posts the answer back. The
// tab itself is drawn by `ui/Browser.tsx` from what is kept here: the address, whether a page is
// loading, and what the agent is doing in it right now.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { post, type Endpoint } from "./api";

export interface Op {
  op: "open" | "read" | "click" | "type" | "key" | "scroll" | "back" | "forward" | "reload";
  url?: string;
  target?: number;
  text?: string;
  submit?: boolean;
  key?: string;
  direction?: "up" | "down";
}

export interface Tab {
  url?: string;
  loading?: boolean;
  /// What the agent did last ("clicking element 12"), and the op it belongs to.
  activity?: string;
  op?: string;
  /// An op is running in the page right now.
  acting?: boolean;
  /// The agent is in a run of browser work: held for `HOLD` after its last op, because it thinks
  /// for seconds between steps and a state cleared after each one read as six blinks, not as
  /// "working". Shown only while the lane's turn runs, so it cannot outlive the agent.
  browsing?: boolean;
  /// Closed by Keel to make room for another lane's page.
  room?: boolean;
  error?: string;
}

const tabs = new Map<string, Tab>();
/// When each lane's agent last acted in its tab, and how long a gap counts as a new run.
const lastOp = new Map<string, number>();
const PAUSE = 30_000;
const HOLD = 8_000;
const listeners = new Set<() => void>();
const put = (lane: string, change: Partial<Tab>) => {
  tabs.set(lane, { ...tabs.get(lane), ...change });
  listeners.forEach((l) => l());
};

export function useTab(lane: string): Tab | undefined {
  return useSyncExternalStore(
    (l) => (listeners.add(l), () => listeners.delete(l)),
    () => tabs.get(lane),
  );
}

/// What the person reads while the agent acts. Never what it typed — how much, and where.
export function activity(op: Op): string {
  switch (op.op) {
    case "open": return `opening ${op.url?.replace(/^https?:\/\//, "")}`;
    case "read": return "analyzing the page";
    case "click": return `clicking element ${op.target}`;
    case "type": return `typing ${op.text?.length ?? 0} characters into element ${op.target}${op.submit ? ", then submitting" : ""}`;
    case "key": return `pressing ${op.key}`;
    case "scroll": return `scrolling ${op.direction}`;
    default: return `going ${op.op}`;
  }
}

/// Run one op in the lane's tab. The person's own back, forward, reload and address go through here
/// too, so they wait their turn behind the agent's.
export function act(lane: string, project: string, op: Op): Promise<string> {
  return invoke<string>("browser_act", { lane, project, op });
}

/// The daemon's `browser` event: run it, answer it. A lane this window does not have is answered
/// at once, so the agent hears why rather than waiting out the call.
export async function onBrowser(project: string, ep: Endpoint, has: (lane: string) => boolean, data: { id: string; lane: string; op: Op }, reveal: (lane: string) => void = () => undefined) {
  const answer = (ok: boolean, text: string) => post(ep, "/api/browser/result", { id: data.id, ok, text }).catch(() => undefined);
  if (!has(data.lane)) return answer(false, "That lane isn't open in Keel, so it has no Browser tab.");
  // The tab comes into view when the agent opens a page, or starts working in the browser after a
  // pause — panel opened if it was closed, Browser tab chosen. "It didn't open the browser" was the
  // agent working in a tab the person had no reason to look at. Not on every action of a run, so
  // the person can look at another tab while it works; and never keyboard focus, which would send
  // what they type into the composer to the website instead.
  const now = Date.now();
  if (data.op.op === "open" || now - (lastOp.get(data.lane) ?? 0) > PAUSE) reveal(data.lane);
  lastOp.set(data.lane, now);
  put(data.lane, { activity: activity(data.op), op: data.id, acting: true, browsing: true, room: false, error: undefined });
  try {
    await answer(true, await act(data.lane, project, data.op));
  } catch (e) {
    await answer(false, e instanceof Error ? e.message : String(e));
  } finally {
    put(data.lane, { acting: false });
    // Only this op's run: the next op may already have started, and holds it again.
    setTimeout(() => { if (tabs.get(data.lane)?.op === data.id) put(data.lane, { browsing: false }); }, HOLD);
  }
}

/// The lane closed, or its project did: its webview goes. Its logins stay with the project.
export function closeTab(lane: string) {
  tabs.delete(lane);
  listeners.forEach((l) => l());
  void invoke("browser_close", { lane }).catch(() => undefined);
}

/// Where each page is, as the webview reports it. Once per window.
let listening = false;
export function listenPages() {
  if (listening) return;
  listening = true;
  void listen<{ lane: string; url?: string; loading?: boolean; closed?: string; error?: string }>("browser-page", ({ payload }) => {
    if (payload.error) put(payload.lane, { url: undefined, loading: false, error: payload.error });
    else if (payload.closed) put(payload.lane, { url: undefined, loading: false, room: true });
    else put(payload.lane, { url: payload.url, loading: !!payload.loading });
  });
}

/// What the person typed into the address field, as an address: a scheme added when there is none.
export function address(typed: string): string {
  const t = typed.trim();
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(t)) return t;
  // A dev server on this machine is plain http; https to it fails after 30 s with no reason.
  return /^(localhost|127\.0\.0\.1|\[::1\])(:|\/|$)/i.test(t) ? `http://${t}` : `https://${t}`;
}

/// How a call is named on screen: the lane's browser tools as what they are, not as an MCP id.
export const shownTool = (tool: string) => (tool.startsWith("mcp__keel__browser_") ? `Browser · ${tool.slice("mcp__keel__browser_".length)}` : tool);

export function setError(lane: string, error?: string) {
  put(lane, { error });
}
