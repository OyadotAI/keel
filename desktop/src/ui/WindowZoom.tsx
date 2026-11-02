// Visible window zoom controls remain outside the footer's scrolling metadata region.
import { useSyncExternalStore } from "react";
import { dismissZoomError, onWindowZoom, retryWindowZoom, RANGES, windowZoomSnapshot, zoomWindow } from "../zoom";
import { label } from "../keys";

/** The keyboard and buttons share applied zoom state while terminal-font shortcuts stay separate. */
export function WindowZoom() {
  const state = useSyncExternalStore(onWindowZoom, windowZoomSnapshot, windowZoomSnapshot);
  const percentage = Math.round(state.level * 100);
  return <div className="window-zoom">
    {state.error && <div className="window-zoom-error" role="alert">{state.error}<button onClick={() => void retryWindowZoom()}>Retry</button><button aria-label="Dismiss zoom error" onClick={dismissZoomError}>×</button></div>}
    <div className="window-zoom-controls" role="group" aria-label="Window zoom"><span>Zoom</span>
      <button aria-label="Zoom window out" title={`Zoom window out (${label({ key: "-" })} outside terminals)`} disabled={state.applying || state.level <= RANGES.APP.min} onClick={() => void zoomWindow(-1)}>−</button>
      <button className="window-zoom-value" aria-label={`Window zoom ${percentage}%. Reset to 100%`} title={`Reset window zoom to 100% (${label({ key: "0" })} outside terminals)`} disabled={state.applying} onClick={() => void zoomWindow(0)}>{percentage}%</button>
      <button aria-label="Zoom window in" title={`Zoom window in (${label({ key: "+" })} outside terminals)`} disabled={state.applying || state.level >= RANGES.APP.max} onClick={() => void zoomWindow(1)}>+</button>
    </div><span className="sr-only" role="status">Window zoom {percentage}%</span>
  </div>;
}
