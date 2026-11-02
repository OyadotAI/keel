// Internal pointer dragging works alongside Tauri's native Windows OS-file drop handler.
import { create } from "zustand";
import { FILE_DRAG_THRESHOLD } from "./chat-constants";
import { fileReference, sameCheckout, type FileReference } from "./chat-files";
import { useStore } from "./store";

interface Drag { path: string; reference: FileReference; x: number; y: number; target?: string }
/** Read-only drag presentation never changes the editor selection or sends a message. */
export const useFileDrag = create<{ drag?: Drag }>(() => ({}));
const suppressed = new WeakSet<EventTarget>();
let stop: (() => void) | undefined;

/** Consume only the synthetic click caused by a completed drag, preserving normal file clicks. */
export function draggedClick(target: EventTarget, detail = 1): boolean {
  if (!detail) return false;
  const dragged = suppressed.has(target);
  suppressed.delete(target);
  return dragged;
}

function targetAt(x: number, y: number): string | undefined {
  return document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-reference-target]")?.dataset.referenceTarget;
}

/** Track one primary-pointer gesture, with a threshold before capture or click suppression. */
export function startFileDrag(lane: string, path: string, event: Pick<PointerEvent, "button" | "isPrimary" | "pointerId" | "clientX" | "clientY"> & { currentTarget: HTMLElement }): void {
  if (event.button !== 0 || !event.isPrimary) return;
  const source = useStore.getState().lanes[lane];
  if (!source) return;
  stop?.();
  const reference = fileReference(source, path);
  const element = event.currentTarget;
  suppressed.delete(element);
  const origin = { x: event.clientX, y: event.clientY };
  let dragging = false;
  const unsubscribe = useStore.subscribe((state) => {
    const current = state.lanes[lane];
    if (!current || !sameCheckout(current, reference)) cancel();
  });
  function cancel() {
    unsubscribe();
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", release);
    window.removeEventListener("pointercancel", cancel);
    window.removeEventListener("keydown", key);
    window.removeEventListener("blur", cancel);
    if (element.hasPointerCapture?.(event.pointerId)) element.releasePointerCapture(event.pointerId);
    useFileDrag.setState({ drag: undefined }); stop = undefined;
  }
  function move(next: PointerEvent) {
    if (next.pointerId !== event.pointerId) return;
    if (!dragging && Math.hypot(next.clientX - origin.x, next.clientY - origin.y) < FILE_DRAG_THRESHOLD) return;
    if (!dragging) { dragging = true; suppressed.add(element); element.setPointerCapture?.(event.pointerId); }
    next.preventDefault();
    useFileDrag.setState({ drag: { path, reference, x: next.clientX, y: next.clientY, target: targetAt(next.clientX, next.clientY) } });
  }
  function release(next: PointerEvent) {
    if (next.pointerId !== event.pointerId) return;
    const target = dragging ? targetAt(next.clientX, next.clientY) : undefined;
    cancel();
    if (target) void useStore.getState().referenceFile(target, reference);
  }
  function key(next: KeyboardEvent) { if (next.key === "Escape") { next.preventDefault(); cancel(); } }
  stop = cancel;
  window.addEventListener("pointermove", move, { passive: false });
  window.addEventListener("pointerup", release);
  window.addEventListener("pointercancel", cancel);
  window.addEventListener("keydown", key);
  window.addEventListener("blur", cancel);
}

/** Tests and an unmounted Explorer can cancel without inventing a drop target. */
export function cancelFileDrag(): void { stop?.(); }
