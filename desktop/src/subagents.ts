// Stable facade shares provider child evidence between terminal panes, lane activity, and members.
export * from "./subagents/model";
export { paneOf, patchPane, patchChildOutput, reconcilePane, useChildPane, releaseGridText } from "./subagents/state";
export type { ChildOutput, PaneState } from "./subagents/state";
export { FOLLOW_DISTANCE, COMPACT_HEIGHT, SHORT_ID } from "./subagents/constants";
export { useCompactChildren, useChildReading } from "./subagents/view";

export { flipFrom, gridColumns, liveInvocations, useDeparting, useFlip, useLiveInvocations, useVisibleChildren, withDeparting } from "./subagents/grid";
export type { Shown } from "./subagents/grid";
