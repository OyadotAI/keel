// The terminal's colours, in the design system: xterm takes concrete hex and no CSS variables, so
// these are the five brand colours flattened by hand. styles.test.mjs proves each one equals the
// token it stands for; nothing outside design/ writes a colour.
const ANSI = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;

// Fixed values are flattened mixes of the same five Oya colors as styles.css. A terminal
// can have a different appearance from its window, so reading window CSS would be incorrect.
const INK = "#1a1a1a", PAPER = "#f7f5f0", LIME = "#cdf01e";
const LIGHT = { text: INK, muted: "#5c5c5a", add: "#57631b", del: "#973c36", warn: "#76592f" };
const DARK = { text: PAPER, muted: "#acaba7", add: LIME, del: "#f48d84", warn: "#f5b14c" };
const PALETTES = {
  light: {
    background: "#eae8e3", foreground: LIGHT.text, cursor: INK,
    selection: "rgba(205, 240, 30, 0.30)", surface: "#e3e1dd", border: "#cdcbc7",
    ansi: [LIGHT.text, LIGHT.del, LIGHT.add, LIGHT.warn, LIGHT.text, INK, LIGHT.muted, LIGHT.muted,
      LIGHT.muted, LIGHT.del, LIGHT.add, LIGHT.warn, LIGHT.text, INK, LIGHT.muted, LIGHT.text],
  },
  dark: {
    background: INK, foreground: DARK.text, cursor: LIME,
    selection: "rgba(205, 240, 30, 0.30)", surface: "#272727", border: "#3d3d3c",
    ansi: [DARK.text, DARK.del, DARK.add, DARK.warn, DARK.text, LIME, DARK.muted, DARK.muted,
      DARK.muted, DARK.del, DARK.add, DARK.warn, DARK.text, LIME, DARK.muted, DARK.text],
  },
};

/** Shared named colors keep transcript panes consistent with the native terminal. */
export function terminalPalette(dark: boolean) {
  const p = dark ? PALETTES.dark : PALETTES.light;
  return { ...p, ...Object.fromEntries(ANSI.map((name, index) => [name, p.ansi[index]])) } as typeof p & Record<typeof ANSI[number], string>;
}

/** Xterm keeps its existing ANSI values while child transcripts reuse the same source. */
export function terminalXtermTheme(dark: boolean) {
  const p = terminalPalette(dark);
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
export function terminalPaneColors(dark: boolean): Record<string, string> {
  const p = terminalPalette(dark);
  return {
    "--child-bg": p.background, "--child-fg": p.foreground, "--child-muted": p.white,
    "--child-tool": p.cyan, "--child-link": p.blue, "--child-keyword": p.magenta,
    "--child-string": p.green, "--child-number": p.yellow, "--child-error": p.red,
    "--child-surface": p.surface, "--child-line": p.border, "--child-selection": p.selection,
  };
}

/** Find-in-terminal highlights: lime for the current match, the add role for the others. */
export function terminalFindDecorations(dark: boolean) {
  const p = terminalPalette(dark);
  return {
    matchOverviewRuler: p.ansi[6],
    activeMatchColorOverviewRuler: LIME,
    matchBackground: p.green,
    activeMatchBackground: LIME,
    activeMatchBorder: p.foreground,
  };
}
