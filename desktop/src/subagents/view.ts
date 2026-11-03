// Geometry and reading positions never send keys to provider processes.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { COMPACT_HEIGHT, FOLLOW_DISTANCE } from "./constants";
import { patchChildOutput, type ChildOutput } from "./state";
/** A short terminal reveals only the header until the person expands it deliberately. */
export function useCompactChildren() {
  const ref = useRef<HTMLElement>(null), [compact, setCompact] = useState(false);
  useEffect(() => {
    const parent = ref.current?.parentElement;
    if (!parent) return;
    const observe = () => setCompact(parent.clientHeight < COMPACT_HEIGHT);
    observe(); const observer = new ResizeObserver(observe); observer.observe(parent);
    return () => observer.disconnect();
  }, []);
  return { ref, compact };
}
/** Reopening restores position; new output follows only while the reader is at the bottom. */
export function useChildReading(scope: string, id: string, output?: ChildOutput) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = ref.current;
    if (element) element.scrollTop = output?.following !== false ? element.scrollHeight : output.top;
  }, [scope, id, output?.detail, output?.following, output?.top]);
  const scroll = () => {
    const element = ref.current;
    if (element) patchChildOutput(scope, id, { top: element.scrollTop, following: element.scrollHeight - element.scrollTop - element.clientHeight <= FOLLOW_DISTANCE });
  };
  const latest = () => { patchChildOutput(scope, id, { following: true }); if (ref.current) ref.current.scrollTop = ref.current.scrollHeight; };
  return { ref, scroll, latest };
}
