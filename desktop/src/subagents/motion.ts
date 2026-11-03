// Transient reveal effects never share transforms with grid movement or filter terminal text.
import { useEffect, useLayoutEffect, useRef, useSyncExternalStore, type RefObject } from "react";
import { PANE_OPEN_MS, PANE_CLOSE_MS, PANE_OPEN_Y, PANE_CLOSE_Y, PANE_REVEAL_OFFSET, PANE_OPEN_EASE, PANE_CLOSE_EASE, REDUCED_MOTION_QUERY } from "./constants";

const stillness = () => typeof matchMedia === "function" && matchMedia(REDUCED_MOTION_QUERY).matches;
function onMotionChange(listener: () => void) {
  if (typeof matchMedia !== "function") return () => {};
  const query = matchMedia(REDUCED_MOTION_QUERY);
  query.addEventListener("change", listener);
  return () => query.removeEventListener("change", listener);
}
/** Live preferences cancel decoration immediately, including already-closing tiles. */
export const useReducedMotion = () => useSyncExternalStore(onMotionChange, stillness, () => false);

/** A single owner samples an interrupted reveal before cancelling and dissolving it. */
export function createPaneMotion() {
  let active: Animation | undefined;
  let wisps: Animation[] = [];
  let node: HTMLElement | undefined;
  let previousLeaving: boolean | undefined;
  const cancel = () => { active?.cancel(); active = undefined; wisps.forEach((wisp) => wisp.cancel()); wisps = []; if (node) { delete node.dataset.motion; node.style.removeProperty("opacity"); node.style.removeProperty("mask-position"); } };
  return {
    cancel,
    play(element: HTMLElement, leaving: boolean, reduced: boolean) {
      const current = active || element.style.opacity ? getComputedStyle(element) : undefined;
      const plumes = [...(element.nextElementSibling?.querySelectorAll<HTMLElement>("i, span") ?? [])];
      const smoke = plumes.map((plume) => current ? { opacity: getComputedStyle(plume).opacity, transform: getComputedStyle(plume).transform } : { opacity: "0", transform: "translate(0, 0) scale(1)" });
      const from = current ? { opacity: current.opacity, transform: current.transform, maskPosition: current.maskPosition, filter: current.filter } : leaving ? { opacity: "1", transform: "none", maskPosition: "0% 0%", filter: "blur(0px)" } : { opacity: "0", transform: `translateY(${PANE_OPEN_Y}px)`, maskPosition: "0% 100%", filter: "blur(0px)" };
      const unchanged = previousLeaving === leaving && !active && !element.style.opacity;
      cancel(); node = element; previousLeaving = leaving;
      if (unchanged || reduced || typeof element.animate !== "function") return;
      element.dataset.motion = leaving ? "closing" : "opening";
      const frames = leaving ? [from, { opacity: "0", transform: `translateY(${PANE_CLOSE_Y}px)`, maskPosition: "0% 100%", filter: "blur(4px)", offset: .65 }, { opacity: "0", transform: `translateY(${PANE_CLOSE_Y}px)`, maskPosition: "0% 100%", filter: "blur(0px)" }] : [from, { opacity: "1", transform: "none", maskPosition: "0% 0%", filter: "blur(0px)", offset: PANE_REVEAL_OFFSET }, { opacity: "1", transform: "none", maskPosition: "0% 0%", filter: "blur(0px)" }];
      const animation = element.animate(frames, { duration: leaving ? PANE_CLOSE_MS : PANE_OPEN_MS, easing: leaving ? PANE_CLOSE_EASE : PANE_OPEN_EASE, fill: "both" });
      active = animation;
      wisps = plumes.map((plume, index) => plume.animate([smoke[index], { opacity: index === 3 ? ".2" : ".6", offset: .25 }, { opacity: "0", transform: `translate(${index === 0 ? -16 : index === 1 ? 16 : 0}px, ${leaving ? -48 : -36}px) scale(${leaving ? 1.16 : 1.12})` }], { duration: leaving ? PANE_CLOSE_MS : PANE_OPEN_MS, easing: "cubic-bezier(.22, .61, .36, 1)", fill: "both" }));
      animation.onfinish = () => { if (active !== animation) return; cancel(); if (leaving) element.style.opacity = "0"; };
    },
  };
}

/** DOM identity remains stable while only its inner visual layer forms or dissolves. */
export function usePaneMotion(ref: RefObject<HTMLElement | null>, leaving: boolean, reduced: boolean): void {
  const motion = useRef<ReturnType<typeof createPaneMotion> | null>(null);
  motion.current ??= createPaneMotion();
  useLayoutEffect(() => { const surface = ref.current?.querySelector<HTMLElement>(".child-terminal-surface"); if (surface) motion.current!.play(surface, leaving, reduced); }, [ref, leaving, reduced]);
  useEffect(() => { const owner = motion.current!; return () => owner.cancel(); }, []);
}
