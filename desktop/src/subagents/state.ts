// Pane preferences survive collapse and lane changes without owning any provider execution.
import { useEffect, useSyncExternalStore } from "react";
import type { ChildDetail, ChildSnapshot } from "./model";
import { OUTPUT_CACHE, PANE_CACHE } from "./constants";
/** A reader can pause following and keep its position independently for each selected invocation. */
export interface ChildOutput { detail?: ChildDetail; disconnected: boolean; top: number; following: boolean }
/** One parent conversation owns its selected child, discovery state, and bounded output history. */
export interface PaneState { open: boolean; deliberate: boolean; revealed: boolean; selected?: string; outputs: Record<string, ChildOutput> }
const panes = new Map<string, PaneState>();
const listeners = new Set<() => void>();
/** Reading a preference never steals focus or starts observation by itself. */
export function paneOf(scope: string): PaneState {
  if (!panes.has(scope)) {
    while (panes.size >= PANE_CACHE) panes.delete(panes.keys().next().value!);
    panes.set(scope, { open: false, deliberate: false, revealed: false, outputs: {} });
  }
  return panes.get(scope)!;
}
/** Updates replace snapshots so mounted and hidden surfaces see the same preferences. */
export function patchPane(scope: string, patch: Partial<PaneState>): void { panes.set(scope, { ...paneOf(scope), ...patch }); listeners.forEach((listener) => listener()); }
/** Initial discovery is automatic once; later activity respects explicit collapse and selection. */
export function reconcilePane(scope: string, snapshot?: ChildSnapshot): void {
  const before = paneOf(scope), children = snapshot?.invocations;
  if (!children?.length) return;
  if (!before.revealed) patchPane(scope, { revealed: true, open: true, selected: children[0].id });
  else if (!children.some((c) => c.id === before.selected)) patchPane(scope, { selected: children[0].id });
}
/** React owns only observation lifetimes; preference state survives component remounts. */
export function useChildPane(scope: string, snapshot?: ChildSnapshot): PaneState {
  useEffect(() => reconcilePane(scope, snapshot), [scope, snapshot]);
  return useSyncExternalStore((listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; }, () => paneOf(scope), () => paneOf(scope));
}
/** A failed output read preserves useful text and its reader position. */
export function patchChildOutput(scope: string, id: string, patch: Partial<ChildOutput>): void {
  const before = paneOf(scope), outputs = { ...before.outputs };
  outputs[id] = { ...(outputs[id] ?? { disconnected: false, top: 0, following: true }), ...patch };
  while (Object.keys(outputs).length > OUTPUT_CACHE) { const oldest = Object.keys(outputs).find((key) => key !== id)!; delete outputs[oldest]; }
  patchPane(scope, { outputs });
}
