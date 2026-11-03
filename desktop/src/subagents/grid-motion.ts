// Grid movement owns one cancellable transform per tile and retargets from its visible geometry.
import { PANE_MOVE_MS, PANE_MOVE_EASE } from "./constants";

/** Layout boxes are untransformed targets; animation progress supplies the visible position. */
export type Box = Pick<DOMRect, "left" | "top" | "width" | "height">;
interface Move { animation: Animation; from: Box; to: Box }
const panes = (root: HTMLElement) => [...root.querySelectorAll<HTMLElement>("[data-pane]")];
const measure = (root: HTMLElement) => new Map(panes(root).map((pane) => [pane.dataset.pane!, pane.getBoundingClientRect()]));
const between = (from: Box, to: Box, progress: number): Box => Object.fromEntries((["left", "top", "width", "height"] as (keyof Box)[]).map((key) => [key, from[key] + (to[key] - from[key]) * progress])) as Box;

/** Inversion preserves the old screen position while the grid has already adopted its new tracks. */
export function flipFrom(was: Box, now: Box): string | undefined {
  if (!now.width || !now.height) return;
  const dx = was.left - now.left, dy = was.top - now.top, sx = was.width / now.width, sy = was.height / now.height;
  if (!dx && !dy && sx === 1 && sy === 1) return;
  return `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`;
}

/** Handles are bounded by mounted tiles and cancelled before measuring new layout targets. */
export function createGridMotion() {
  let boxes = new Map<string, Box>();
  const moves = new Map<string, Move>();
  let contentBox: Box | undefined, release = () => {};
  let held = false;
  const unlock = () => { release(); release = () => {}; held = false; };
  const cancel = (settle = true) => { moves.forEach(({ animation }) => animation.cancel()); moves.clear(); if (settle) unlock(); };
  return {
    cancel: () => cancel(),
    reset(root: HTMLElement) { cancel(); boxes = measure(root); contentBox = root.querySelector<HTMLElement>("[data-main-content]")?.getBoundingClientRect(); },
    update(root: HTMLElement, reduced: boolean) {
      const visible = new Map(boxes);
      moves.forEach(({ animation, from, to }, id) => {
        const progress = animation.effect?.getComputedTiming().progress;
        visible.set(id, typeof progress === "number" ? between(from, to, progress) : to);
      });
      cancel(false);
      boxes = measure(root);
      const content = root.querySelector<HTMLElement>("[data-main-content]");
      if (content && !contentBox) contentBox = content.getBoundingClientRect();
      if (reduced) { unlock(); if (content) contentBox = content.getBoundingClientRect(); return; }
      let movingMain = false;
      for (const pane of panes(root)) {
        const id = pane.dataset.pane!, from = visible.get(id), to = boxes.get(id)!;
        const transform = from && flipFrom(from, to);
        if (!transform || typeof pane.animate !== "function") continue;
        const viewport = pane.querySelector<HTMLElement>("[data-main-viewport]");
        let frames: Keyframe[] = [{ transformOrigin: "0 0", transform }, { transformOrigin: "0 0", transform: "none" }];
        if (viewport) {
          movingMain = true;
          if (content && contentBox && !held) {
            const { width, height, flex } = content.style;
            content.style.width = contentBox.width + "px"; content.style.height = contentBox.height + "px"; content.style.flex = "none";
            held = true;
            release = () => { Object.assign(content.style, { width, height, flex }); contentBox = content.getBoundingClientRect(); delete viewport.dataset.moving; };
          }
          viewport.dataset.moving = "true";
          frames = [{ transform: `translate(${from!.left - to.left}px, ${from!.top - to.top}px)`, width: from!.width + "px", height: from!.height + "px" }, { transform: "none", width: to.width + "px", height: to.height + "px" }];
        }
        const animation = (viewport ?? pane).animate(frames, { duration: PANE_MOVE_MS, easing: PANE_MOVE_EASE, fill: "both" });
        moves.set(id, { animation, from: from!, to });
        animation.onfinish = () => { if (moves.get(id)?.animation === animation) { moves.delete(id); animation.cancel(); if (viewport) unlock(); } };
      }
      if (!movingMain) { unlock(); if (content) contentBox = content.getBoundingClientRect(); }
    },
  };
}
