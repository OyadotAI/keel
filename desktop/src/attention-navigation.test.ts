// Navigation tests pin source ownership, stale-row recovery and finite focus lifetime.
import { expect, it, vi } from "vitest";
import { activateAttention, attentionLayout, deferAttentionFocus, revealAttention } from "./attention-navigation";
import type { AttentionSource } from "./attention";
import type { Lane } from "./store";

const base: Lane = { id: "lane", project: "/p", title: "Task", agent: "codex", interface: "terminal", known: true, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [], unseen: true };
const source = (lane: Lane = base): AttentionSource => ({ order: ["/p"], projects: { "/p": { name: "Project", lanes: [lane.id] } }, lanes: { [lane.id]: lane } });

it.each(["checks", "update"] as const)("%s uncovers Turns instead of preserving a narrow conversation overlay or old chat scroll", (kind) => {
  expect(attentionLayout({ kind })).toEqual({ panel: true, conversation: false, expanded: false, tab: "turns", hideSidebar: false });
});

it.each([{ kind: "request", request: "p" } as const, { kind: "terminal" } as const, { kind: "error" } as const])("%s closes covering workspace panels before revealing the conversation", (destination) => {
  expect(attentionLayout(destination)).toEqual({ panel: false, conversation: true, expanded: false, tab: "turns", hideSidebar: false });
});

it.each(["checks", "update", "error", "terminal"] as const)("%s closes a covering narrow drawer without hiding the desktop sidebar", (kind) => {
  expect(attentionLayout({ kind }, true).hideSidebar).toBe(true);
  expect(attentionLayout({ kind }, false).hideSidebar).toBe(false);
});

it("replaces the App's covering state and preserves its desktop sidebar preference", () => {
  const view = { panel: true, conversation: true, expanded: true, tab: "browser", sidebar: true };
  const actions = { panel: (value: boolean) => { view.panel = value; }, conversation: (value: boolean) => { view.conversation = value; }, expanded: (value: boolean) => { view.expanded = value; }, tab: (value: "turns") => { view.tab = value; }, sidebar: (value: boolean) => { view.sidebar = value; } };
  revealAttention({ kind: "request", request: "q" }, actions, true);
  expect(view).toEqual({ panel: false, conversation: true, expanded: false, tab: "turns", sidebar: false });
  view.sidebar = true;
  revealAttention({ kind: "checks" }, actions, false);
  expect(view).toEqual({ panel: true, conversation: false, expanded: false, tab: "turns", sidebar: true });
});

it("recomputes the current destination and leaves source resolution to existing selection", () => {
  const state = source({ ...base, attention: { id: "prompt", kind: "question" }, error: "delivery error" });
  const select = vi.fn((id: string) => { state.lanes[id] = { ...state.lanes[id]!, unseen: false }; });
  const reveal = vi.fn();
  expect(activateAttention("lane", { read: () => state, select, reveal })).toEqual({ kind: "terminal" });
  expect(reveal).toHaveBeenCalledWith("lane", { kind: "terminal" });
  expect(state.lanes.lane).toMatchObject({ unseen: false, attention: { id: "prompt" }, error: "delivery error", interface: "terminal" });
  state.lanes.lane = { ...state.lanes.lane!, attention: undefined };
  expect(activateAttention("lane", { read: () => state, select, reveal })).toEqual({ kind: "error" });
});

it("a removed project or resolved row cannot select a lane from an obsolete callback", () => {
  const state = source(), select = vi.fn(), reveal = vi.fn();
  state.order = [];
  expect(activateAttention("lane", { read: () => state, select, reveal })).toBeUndefined();
  state.order = ["/p"];
  state.lanes.lane = { ...base, unseen: false };
  expect(activateAttention("lane", { read: () => state, select, reveal })).toBeUndefined();
  expect(select).not.toHaveBeenCalled(); expect(reveal).not.toHaveBeenCalled();
});

function frames() {
  let next = 0;
  const callbacks = new Map<number, () => void>();
  return { frame: (callback: () => void) => { callbacks.set(++next, callback); return next; }, cancel: (id: number) => { callbacks.delete(id); }, run: () => { const pending = [...callbacks.values()]; callbacks.clear(); pending.forEach((callback) => callback()); }, pending: () => callbacks.size };
}

it("waits for a lazy destination, stops after success and cancels without leaking work", () => {
  const clock = frames(), focus = vi.fn().mockReturnValueOnce(false).mockReturnValue(true), fallback = vi.fn();
  deferAttentionFocus({ ...clock, current: () => true, focus, fallback });
  clock.run(); clock.run(); clock.run();
  expect(focus).toHaveBeenCalledTimes(2); expect(fallback).not.toHaveBeenCalled(); expect(clock.pending()).toBe(0);
  const stop = deferAttentionFocus({ ...clock, current: () => true, focus, fallback });
  stop(); clock.run(); expect(focus).toHaveBeenCalledTimes(2);
});

it("bounds an unavailable destination and never focuses after the active navigation changes", () => {
  const clock = frames(), fallback = vi.fn(), focus = vi.fn(() => false);
  deferAttentionFocus({ ...clock, current: () => true, focus, fallback });
  for (let count = 0; count < 100; count++) clock.run();
  expect(fallback).toHaveBeenCalledTimes(1); expect(clock.pending()).toBe(0);
  focus.mockClear(); fallback.mockClear();
  deferAttentionFocus({ ...clock, current: () => false, focus, fallback });
  clock.run(); expect(focus).not.toHaveBeenCalled(); expect(fallback).not.toHaveBeenCalled(); expect(clock.pending()).toBe(0);
});
