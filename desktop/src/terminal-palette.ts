// One palette owns main-terminal ANSI colors and read-only child transcript colors.
const ANSI = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;

/// Keel's two terminal palettes: background, text and all sixteen ANSI colours, from the same
/// tokens as the window. Fixed values rather than read from the page, because the terminal's
/// colours are a choice of their own (`terminal-theme`), which may differ from the window's.
const PALETTES = {
  light: {
    background: "#fafaf8",
    foreground: "#1c1c1a",
    cursor: "#157a13",
    selection: "rgba(21, 122, 19, 0.25)",
    surface: "#f0efeb",
    border: "#dedcd4",
    ansi: ["#1c1c1a", "#c0322a", "#16774a", "#9a5200", "#2f5fb3", "#8a3fa0", "#1b7a86", "#62625a", "#45453e", "#d5473e", "#157a13", "#b06a12", "#3d6fc7", "#9d52b3", "#238e9b", "#1c1c1a"],
  },
  dark: {
    background: "#141410",
    foreground: "#f4f3ef",
    cursor: "#39ed35",
    selection: "rgba(57, 237, 53, 0.25)",
    surface: "#1c1c18",
    border: "#2c2c26",
    ansi: ["#2c2c26", "#ff7b72", "#5cd39a", "#f2b544", "#79a8ff", "#d199ff", "#6fd3dd", "#babab0", "#62625a", "#ff9a92", "#7fe0b0", "#f7c96b", "#9cbfff", "#ddb3ff", "#93e2ea", "#f4f3ef"],
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
