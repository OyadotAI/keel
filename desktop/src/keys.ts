// Keel's shortcuts, and the one rule that matters about them: the terminal owns every key that
// is not on this list. Esc, ⌃C, ⇧⇥ are Claude Code's.
//
// ⌘ on macOS. Elsewhere Ctrl alone belongs to the terminal (⌃C must reach the agent), so ⌘ is
// Ctrl+Shift there and ⌘⇧ is Ctrl+Shift+Alt — the convention Windows Terminal uses.
//
// Matched on the physical key (`e.code`), not the character: with Shift held `]` arrives as
// `}`, and with Option held on a Mac a letter arrives as a dead key or an accent.

export const MAC = /Mac|iPhone|iPad/.test(navigator.platform);

export interface Chord {
  key: string;
  shift?: boolean;
  alt?: boolean;
}

const CODES: Record<string, string> = { "[": "BracketLeft", "]": "BracketRight", ",": "Comma", "'": "Quote", Backspace: "Backspace", ArrowUp: "ArrowUp", ArrowDown: "ArrowDown" };
const GLYPHS: Record<string, string> = { Backspace: "⌫", ArrowUp: "↑", ArrowDown: "↓" };

function code(key: string): string {
  if (CODES[key]) return CODES[key];
  if (/^[0-9]$/.test(key)) return `Digit${key}`;
  return `Key${key.toUpperCase()}`;
}

export function matches(e: KeyboardEvent, c: Chord): boolean {
  if (e.code !== code(c.key)) return false;
  if (MAC) return e.metaKey && !e.ctrlKey && e.shiftKey === !!c.shift && e.altKey === !!c.alt;
  // Ctrl+Shift is ⌘; adding Shift or Option to a chord adds Alt.
  return e.ctrlKey && e.shiftKey && e.altKey === !!(c.shift || c.alt) && !e.metaKey;
}

export function label(c: Chord): string {
  if (MAC) return `${c.alt ? "⌥" : ""}${c.shift ? "⇧" : ""}⌘${c.key.length === 1 ? c.key.toUpperCase() : (GLYPHS[c.key] ?? c.key)}`;
  return `Ctrl+Shift+${c.shift || c.alt ? "Alt+" : ""}${c.key.length === 1 ? c.key.toUpperCase() : c.key}`;
}

/// Every chord Keel answers — what the terminal hands up instead of typing. Exactly these and
/// nothing else, so a key the CLI uses (Ctrl+_ for undo, say) still reaches it.
export const KEEL: Chord[] = [
  { key: "k" },
  { key: "o" },
  { key: "l" },
  { key: "j" },
  { key: "n" },
  { key: "w" },
  { key: "," },
  { key: "e", shift: true },
  { key: "i", alt: true },
  { key: "n", shift: true },
  { key: "a", shift: true },
  { key: "d", shift: true },
  { key: "m", shift: true },
  { key: "Backspace", shift: true },
  { key: "]", shift: true },
  { key: "[", shift: true },
  { key: "ArrowUp", alt: true },
  { key: "ArrowDown", alt: true },
  ...["1", "2", "3", "4", "5", "6", "7", "8", "9"].map((key) => ({ key })),
  ...["1", "2", "3", "4", "5", "6"].map((key) => ({ key, alt: true })),
];

export function isKeel(e: KeyboardEvent): boolean {
  return KEEL.some((c) => matches(e, c));
}
