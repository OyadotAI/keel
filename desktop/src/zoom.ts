// ⌘+ / ⌘− / ⌘0 change what has focus: the terminal's font when a terminal does, the whole window
// otherwise. One chord for both, because which one a person means is the one they are looking at.
// Kept in localStorage like the chat interface choice: a per-machine preference, not project state.

import { getCurrentWebview } from "@tauri-apps/api/webview";

const APP = { key: "keel.zoom", min: 0.5, max: 2, step: 0.1, base: 1 };
const FONT = { key: "keel.terminal-font", min: 9, max: 28, step: 1, base: 13 };
type Range = typeof APP;

const clamp = (r: Range, v: number) => Math.round(Math.min(r.max, Math.max(r.min, v)) * 100) / 100;

function read(r: Range): number {
  try {
    const v = Number(localStorage.getItem(r.key));
    return v ? clamp(r, v) : r.base;
  } catch {
    return r.base;
  }
}

function write(r: Range, v: number) {
  try {
    localStorage.setItem(r.key, String(v));
  } catch {
    /* this session still has it */
  }
}

/// The next value for `dir`: +1 larger, −1 smaller, 0 back to the default.
export const next = (r: Range, now: number, dir: -1 | 0 | 1) => (dir === 0 ? r.base : clamp(r, now + dir * r.step));

const fonts = new Set<(size: number) => void>();
export const terminalFont = () => read(FONT);
/// Every live terminal listens, so a change in one is the size of all of them — they are one setting.
export function onTerminalFont(f: (size: number) => void): () => void {
  fonts.add(f);
  return () => fonts.delete(f);
}

const applyApp = (level: number) => void getCurrentWebview().setZoom(level).catch(() => undefined);

/// The saved window zoom, once at launch.
export function restoreZoom() {
  const level = read(APP);
  if (level !== APP.base) applyApp(level);
}

/// Whether focus is inside a terminal: the chord then sizes its font rather than the window.
export const inTerminal = (el: Element | null) => !!el?.closest(".xterm");

export function zoom(dir: -1 | 0 | 1, focused: Element | null = document.activeElement) {
  if (inTerminal(focused)) {
    const size = next(FONT, read(FONT), dir);
    write(FONT, size);
    fonts.forEach((f) => f(size));
  } else {
    const level = next(APP, read(APP), dir);
    write(APP, level);
    applyApp(level);
  }
}

export const RANGES = { APP, FONT };

/// The window's zoom, for anything native laid over the page: a child webview is placed in the
/// window's own units, the page measures in its CSS pixels, and at any zoom but 100% the two
/// drift apart — the Browser tab's page sat over its own address bar.
export const appZoom = () => read(APP);

/// A page rectangle in the window's units.
export const toWindow = (r: DOMRect) => {
  const z = appZoom();
  return { x: r.left * z, y: r.top * z, width: r.width * z, height: r.height * z };
};
