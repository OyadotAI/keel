// Stable pre-PR facade keeps existing callers while state and operations share one owner.
export type * from "./prereview/model";
export { bindReview, reviewOf, useReview, refreshReview, updateReviewForm, reviewForm } from "./prereview/state";
export { startReview, stopReview, dismissReview, sendFindings, openPullRequest } from "./prereview/actions";
export { prKey, usePrRun, prForTarget } from "./prereview/opening";
export type { PrRun } from "./prereview/opening";
export { baseOf, prLink, prLabel, sorted, navigable, fixPrompt, clock } from "./prereview/format";
export { verdictPresentation, formPresentation } from "./prereview/presentation";
