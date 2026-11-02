// Native overlays and responsive sidebars use the footer's actual wrapped height.
import { useLayoutEffect, type RefObject } from "react";

/** Intrinsic footer layout drives a CSS variable without feeding that height back into itself. */
export function useFooterHeight(ref: RefObject<HTMLElement | null>): void {
  useLayoutEffect(() => {
    const footer = ref.current;
    if (!footer) return;
    let frame = 0;
    const measure = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(() => document.documentElement.style.setProperty("--statusbar-h", `${Math.ceil(footer.getBoundingClientRect().height)}px`)); };
    const observer = new ResizeObserver(measure); observer.observe(footer); measure();
    return () => { observer.disconnect(); cancelAnimationFrame(frame); };
  }, [ref]);
}
