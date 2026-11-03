// Bounds keep hidden-lane preferences and selected output caches from retaining unlimited history.
export const PANE_CACHE = 128;
export const OUTPUT_CACHE = 128;
export const OUTPUT_CACHE_BYTES = 2 * 1024 * 1024;
export const GRID_BATCH = 8;
export const GRID_POLL_MS = 1000;
export const GRID_REQUEST_MS = 8000;
export const GRID_MIN_WIDTH = 280;
export const GRID_MAX_COLUMNS = 3;
/** How long a finished pane stays for its close animation, matching `pane-close` in SubagentsPane.css. */
export const PANE_CLOSE_MS = 260;
/** How long a pane takes to glide from its old box to its new one when the grid retracks. */
export const PANE_MOVE_MS = 320;
export const RETRY_FIRST = 1000;
export const RETRY_MAX = 15_000;
export const RETRY_FACTOR = 2;
export const FOLLOW_DISTANCE = 24;
export const COMPACT_HEIGHT = 460;
export const SHORT_ID = 8;
