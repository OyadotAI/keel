// Pointer resize ownership has one idempotent cleanup path, including collapse and lost capture.
import { type cardLayout, CARD_KEY_STEP, CARD_KEY_FAST_STEP, CARD_SPLIT_DEFAULT } from "./git-card-layout";
type Layout = ReturnType<typeof cardLayout>;
interface Dependencies { region(): { top: number } | undefined; layout(): Layout; preview(share: number | undefined): void; commit(share: number): void }

/** Resizing starts at the displayed position, rather than jumping to a constrained saved ratio. */
export function startCardResize(event: Pick<PointerEvent, "button" | "isPrimary" | "pointerId" | "clientY" | "preventDefault"> & { currentTarget: HTMLElement }, deps: Dependencies): () => void {
  const region = deps.region();
  if (event.button !== 0 || !event.isPrimary || !region) return () => {};
  event.preventDefault();
  const handle = event.currentTarget, id = event.pointerId, offset = event.clientY - region.top - deps.layout().upper;
  const body = document.body.style, oldSelect = body.userSelect, oldCursor = body.cursor;
  body.userSelect = "none"; body.cursor = "ns-resize";
  let frame = 0, point: number | undefined, chosen: number | undefined, done = false;
  function apply() {
    frame = 0; const current = deps.region();
    if (point === undefined || !current) return;
    const view = deps.layout(), ratio = (point - current.top - offset) / view.usable;
    chosen = Math.max(view.minimum, Math.min(view.maximum, ratio)); deps.preview(chosen);
  }
  function move(next: PointerEvent) { if (next.pointerId === id) { next.preventDefault(); point = next.clientY; if (!frame) frame = requestAnimationFrame(apply); } }
  function finish() {
    if (done) return; done = true; cancelAnimationFrame(frame); apply();
    window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", end); window.removeEventListener("pointercancel", end); window.removeEventListener("blur", finish); handle.removeEventListener("lostpointercapture", finish);
    if (handle.hasPointerCapture(id)) handle.releasePointerCapture(id);
    body.userSelect = oldSelect; body.cursor = oldCursor;
    if (chosen !== undefined) deps.commit(chosen);
    deps.preview(undefined);
  }
  function end(next: PointerEvent) { if (next.pointerId === id) finish(); }
  try { handle.setPointerCapture(id); } catch { finish(); return () => {}; }
  handle.addEventListener("lostpointercapture", finish);
  window.addEventListener("pointermove", move, { passive: false }); window.addEventListener("pointerup", end); window.addEventListener("pointercancel", end); window.addEventListener("blur", finish);
  return finish;
}

/** Keyboard commands change only the split preference and prevent scrolling when handled. */
export function cardResizeKey(view: Layout, key: string, shift: boolean): number | undefined {
  if (key === "Home") return CARD_SPLIT_DEFAULT;
  if (key !== "ArrowUp" && key !== "ArrowDown") return;
  const now = view.upper / view.usable, direction = key === "ArrowUp" ? -1 : 1;
  const next = Math.max(view.minimum, Math.min(view.maximum, now + direction * (shift ? CARD_KEY_FAST_STEP : CARD_KEY_STEP)));
  return next === now ? undefined : next;
}
