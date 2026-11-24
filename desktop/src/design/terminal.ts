// The terminal's colours, in the design system: xterm takes concrete hex and no CSS variables, so
// these are the five brand colours flattened by hand. styles.test.mjs proves each one equals the
// token it stands for; nothing outside design/ writes a colour.
const ANSI = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;

// Flattened mixes of the five Oya colours. The app is dark only; so is every terminal.
const INK = "#1a1a1a", PAPER = "#f7f5f0", LIME = "#cdf01e";
const DARK = { text: PAPER, muted: "#acaba7", add: LIME, del: "#f48d84", warn: "#f5b14c" };
const PALETTE = {
  background: INK, foreground: DARK.text, cursor: LIME,
  selection: "rgba(205, 240, 30, 0.30)", surface: "#272727", border: "#3d3d3c",
  ansi: [DARK.text, DARK.del, DARK.add, DARK.warn, DARK.text, LIME, DARK.muted, DARK.muted,
      DARK.muted, DARK.del, DARK.add, DARK.warn, DARK.text, LIME, DARK.muted, DARK.text],
};

/** Shared named colors keep transcript panes consistent with the native terminal. */
export function terminalPalette() {
  const p = PALETTE;
  return { ...p, ...Object.fromEntries(ANSI.map((name, index) => [name, p.ansi[index]])) } as typeof p & Record<typeof ANSI[number], string>;
}

/** Xterm keeps its existing ANSI values while child transcripts reuse the same source. */
export function terminalXtermTheme() {
  const p = terminalPalette();
  const out: Record<string, string> = {
  background: p.background,
    foreground: p.foreground,
    cursor: p.cursor,
    cursorAccent: p.background,
  selectionBackground: p.selection,
  };
  ANSI.forEach((name, i) => {
    out[name] = p.ansi[i];
    out[`bright${name[0].toUpperCase()}${name.slice(1)}`] = p.ansi[i + 8];
  });
  return out;
}

/** Terminal-owned CSS tokens prevent Markdown and controls inheriting the window's theme. */
export function terminalPaneColors(): Record<string, string> {
  const p = terminalPalette();
  return {
    "--child-bg": p.background, "--child-fg": p.foreground, "--child-muted": p.white,
    "--child-tool": p.cyan, "--child-link": p.blue, "--child-keyword": p.magenta,
    "--child-string": p.green, "--child-number": p.yellow, "--child-error": p.red,
    "--child-surface": p.surface, "--child-line": p.border, "--child-selection": p.selection,
  };
}

/** Find-in-terminal highlights: lime for the current match, the add role for the others. */
export function terminalFindDecorations() {
  const p = terminalPalette();
  return {
    matchOverviewRuler: p.ansi[6],
    activeMatchColorOverviewRuler: LIME,
    matchBackground: p.green,
    activeMatchBackground: LIME,
    activeMatchBorder: p.foreground,
  };
}
