// IDE state is independent of the agent interface. A layout switch never hands off a session.
import { create } from "zustand";
import type { GraphCommit } from "./commit-graph";
import type { Conversation } from "./reduce";
import { isMarkdown } from "./markdown";

export interface FileNode { name: string; path: string; dir: boolean; children?: FileNode[] | null }
export type FileMode = "code" | "diff";
export type TabClose = "current" | "left" | "right" | "others";
export interface IdeLane {
  tabs: string[];
  selected?: string;
  commit?: GraphCommit;
  mode: FileMode;
  markdown?: Record<string, "source" | "preview" | "split">;
  previewAnchor?: string;
  staged?: boolean;
  targetLine?: { line: number; token: number };
  follow: boolean;
  expanded: string[];
  scroll: Record<string, number>;
}
const empty = (): IdeLane => ({ tabs: [], mode: "code", follow: true, expanded: [], scroll: {} });
export const EMPTY_IDE: IdeLane = empty();
export function rememberedLayout(): "chat" | "ide" {
  try { return localStorage.getItem("keel.workspace-layout") === "ide" ? "ide" : "chat"; }
  catch { return "chat"; }
}

export const useIde = create<{
  layout: "chat" | "ide";
  lanes: Record<string, IdeLane>;
  sidebar: "explorer" | "git" | "conversations";
  navigation?: "files" | "line" | "find";
  setSidebar(sidebar: "explorer" | "git" | "conversations"): void;
  navigate(lane: string, path: string, line?: number): void;
  markdownView(lane: string, view?: "source" | "preview" | "split"): boolean;
  setLayout(layout: "chat" | "ide"): void;
  patch(lane: string, patch: Partial<IdeLane>): void;
  open(lane: string, path: string, mode: FileMode, manual?: boolean, staged?: boolean): void;
  close(lane: string, path: string | null, side?: TabClose): void;
}>()((set) => ({
  layout: rememberedLayout(), lanes: {}, sidebar: "explorer",
  setSidebar(sidebar) { set({ sidebar }); },
  navigate(lane, path, line) {
    useIde.getState().open(lane, path, "code");
    if (line && isMarkdown(path)) useIde.getState().markdownView(lane, "source");
    useIde.getState().patch(lane, { targetLine: { line: line ?? 1, token: Date.now() } });
  },
  markdownView(lane, view) {
    const l = useIde.getState().lanes[lane];
    if (!l?.selected || l.commit || !isMarkdown(l.selected)) return false;
    const next = view ?? (l.markdown?.[l.selected] === "preview" ? "source" : "preview");
    useIde.getState().patch(lane, { mode: "code", follow: false, markdown: { ...l.markdown, [l.selected]: next } });
    return true;
  },
  setLayout(layout) {
    try { localStorage.setItem("keel.workspace-layout", layout); } catch { /* Optional preference. */ }
    set({ layout });
  },
  patch(lane, patch) { set((s) => ({ lanes: { ...s.lanes, [lane]: { ...(s.lanes[lane] ?? empty()), ...patch } } })); },
  open(lane, path, mode, manual = true, staged) {
    set((s) => {
      const l = s.lanes[lane] ?? empty();
      const parents = path.split("/").slice(0, -1).map((_, i, parts) => parts.slice(0, i + 1).join("/"));
      return { lanes: { ...s.lanes, [lane]: { ...l, selected: path, commit: undefined, previewAnchor: undefined, mode, staged, targetLine: undefined, follow: manual ? false : l.follow,
        tabs: l.tabs.includes(path) ? l.tabs : [...l.tabs, path], expanded: [...new Set([...l.expanded, ...parents])] } } };
    });
  },
  close(lane, path, side = "current") {
    set((s) => ({ lanes: { ...s.lanes, [lane]: remainingTabs(s.lanes[lane] ?? empty(), path, side) } }));
  },
}));

/** Include the commit-details tab in the same visual left/right order as file tabs. */
const keepTab: Record<TabClose, (index: number, target: number) => boolean> = {
  current: (i, at) => i !== at,
  left: (i, at) => i >= at,
  right: (i, at) => i <= at,
  others: (i, at) => i === at,
};
function remainingTabs(l: IdeLane, path: string | null, side: TabClose): IdeLane {
  const at = path === null ? l.commit ? l.tabs.length : -1 : l.tabs.indexOf(path);
  if (at < 0) return l;
  const keep = keepTab[side];
  const tabs = l.tabs.filter((_, i) => keep(i, at));
  const selected = l.selected && tabs.includes(l.selected) ? l.selected : path && tabs.includes(path) ? path : tabs[Math.max(0, l.tabs.indexOf(l.selected ?? "") - 1)];
  const commit = l.commit && keep(l.tabs.length, at) ? l.commit : undefined;
  return { ...l, tabs, selected, commit, follow: false, targetLine: selected === l.selected ? l.targetLine : undefined, previewAnchor: undefined };
}

/** Only completed successful edits move the reader. Include child calls, for either provider. */
export function completedEdits(conv: Conversation): { id: string; path: string }[] {
  const turn = conv.turns.at(-1);
  return turn ? Object.values(turn.calls).filter((c) => c.state === "ok").flatMap((c) => c.writes.map((path) => ({ id: `${turn.id}:${c.id}:${path}`, path }))) : [];
}

export function relativeFile(path: string, root: string): string | undefined {
  path = path.replace(/\\/g, "/");
  const prefix = root.replace(/\\/g, "/").replace(/\/$/, "") + "/";
  const windows = /^[a-z]:\//i.test(prefix);
  const inside = windows ? path.toLowerCase().startsWith(prefix.toLowerCase()) : path.startsWith(prefix);
  const relative = inside ? path.slice(prefix.length) : path;
  if (!relative || relative.startsWith("/") || /^[a-z]:[\\/]/i.test(relative) || relative.split(/[\\/]/).includes("..")) return;
  return relative.replace(/\\/g, "/");
}

export interface TreeRow { node: FileNode; depth: number; parent?: string }
export function visibleTree(nodes: FileNode[], expanded: string[], changed: string[] = []): TreeRow[] {
  // Deleted/ignored changed paths still need to be discoverable without entering the file tree.
  const paths = new Set<string>();
  const collect = (list: FileNode[]) => { for (const n of list) { paths.add(n.path); if (n.children) collect(n.children); } };
  collect(nodes);
  const extra = changed.filter((p) => !paths.has(p)).map((path) => ({ name: path, path, dir: false }));
  const open = new Set(expanded);
  const out: TreeRow[] = [];
  const visit = (list: FileNode[], depth: number, parent?: string) => {
    for (const node of list) {
      out.push({ node, depth, parent });
      if (node.children && open.has(node.path)) visit(node.children, depth + 1, node.path);
    }
  };
  visit([...nodes, ...extra], 0);
  return out;
}

/** Coalesce events without cancelling a useful in-flight read. Closing invalidates old contexts. */
export function liveReader<T>(read: () => Promise<T>, accept: (value: T) => void, reject: (error: unknown) => void, delay = 150) {
  let closed = false;
  let busy = false;
  let again = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const refresh = () => {
    if (closed) return;
    if (busy) { again = true; return; }
    if (timer) return;
    timer = setTimeout(() => {
      timer = undefined;
      busy = true;
      void read().then((value) => { if (!closed) accept(value); }, (error) => { if (!closed) reject(error); }).finally(() => {
        busy = false;
        if (again) { again = false; refresh(); }
      });
    }, delay);
  };
  return { refresh, close() { closed = true; clearTimeout(timer); } };
}
