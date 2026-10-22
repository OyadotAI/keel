// The agent terminals' colours: the window's, or light or dark whatever the window is. One
// setting for every terminal and every agent — Codex and agy ask the terminal for its background
// when they start, and Claude Code is started in the matching theme (`theme=` on the PTY), so
// all three draw for the background they get. Kept in localStorage like the window's appearance:
// a per-machine preference, not project state.

import { useSyncExternalStore } from "react";
import { isDark, onAppearance } from "./appearance";

export type TerminalTheme = "window" | "light" | "dark";
const KEY = "keel:terminal-theme";
const listeners = new Set<() => void>();

function read(): TerminalTheme {
  try {
    const v = localStorage.getItem(KEY);
    return v === "light" || v === "dark" ? v : "window";
  } catch {
    return "window";
  }
}

let mode = read();

export function setTerminalTheme(value: TerminalTheme) {
  mode = value;
  try { localStorage.setItem(KEY, value); } catch { /* this launch still has it */ }
  listeners.forEach((l) => l());
}

export const terminalTheme = () => mode;
/// Whether terminals are dark now: the choice, or the window's when the choice is to follow it.
export const terminalDark = () => (mode === "window" ? isDark() : mode === "dark");

/// Called when what `terminalDark` answers may have changed — the choice, or the window's.
export function onTerminalTheme(listener: () => void): () => void {
  listeners.add(listener);
  const off = onAppearance(listener);
  return () => { listeners.delete(listener); off(); };
}

export const useTerminalTheme = () => useSyncExternalStore((l) => onTerminalTheme(l), terminalTheme);
