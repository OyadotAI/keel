// Shared colors must remain readable on terminal-owned surfaces independently of window theme.
import { expect, it } from "vitest";
import { terminalPalette, terminalPaneColors, terminalXtermTheme } from "./terminal-palette";

const luminance = (hex: string) => {
  const channels = hex.slice(1).match(/../g)!.map((part) => parseInt(part, 16) / 255).map((value) => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4);
  return channels[0] * .2126 + channels[1] * .7152 + channels[2] * .0722;
};
const contrast = (a: string, b: string) => (Math.max(luminance(a), luminance(b)) + .05) / (Math.min(luminance(a), luminance(b)) + .05);

for (const dark of [false, true]) {
  it(`${dark ? "dark" : "light"} transcripts reuse the native terminal palette`, () => {
    const p = terminalPalette(dark), css = terminalPaneColors(dark), xterm = terminalXtermTheme(dark);
    expect(css["--child-bg"]).toBe(xterm.background);
    expect(css["--child-fg"]).toBe(xterm.foreground);
    expect(css["--child-tool"]).toBe(xterm.cyan);
    expect(css["--child-link"]).toBe(xterm.blue);
    expect(css["--child-keyword"]).toBe(xterm.magenta);
    expect(css["--child-muted"]).toBe(xterm.white);
    expect(xterm.brightGreen).toBe(p.ansi[10]);
  });
  it(`${dark ? "dark" : "light"} transcript text meets contrast on each surface where it is used`, () => {
    const css = terminalPaneColors(dark);
    const colors = ["--child-fg", "--child-muted", "--child-link", "--child-keyword", "--child-string", "--child-number", "--child-error"];
    for (const surface of ["--child-bg", "--child-surface"]) {
      for (const color of colors) expect(contrast(css[color], css[surface]), `${color} on ${surface}`).toBeGreaterThanOrEqual(4.5);
    }
    expect(contrast(css["--child-tool"], css["--child-bg"])).toBeGreaterThanOrEqual(4.5);
  });
}
