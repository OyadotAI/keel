// PR state has bounded retention, cancellation waits, and diagnostic output.
export const FORM_KEY = "keel.pr.forms";
export const FORM_LIMIT = 64;
export const STOP_WAIT = 10_000;
export const STOP_POLL = 100;
export const LOG_LINES = 100;
export const MIN_REQUEST_WAIT = 1;
