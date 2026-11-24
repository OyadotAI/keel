// Pin shared brand roles and readable colors across the actual stylesheet cascade and terminals.
import { expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { URL } from "node:url";
const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const styles = read("./design/tokens.css") + read("./styles.css"), workspace = read("./workspace.css"), ide = read("./ide.css");
const website = read("../../website/public/tokens.css");
import { terminalPalette } from "./design/terminal";

const BASES = ["#1a1a1a", "#f7f5f0", "#cdf01e", "#f2554a", "#f5b14c"];
const channels = (hex) => hex.slice(1).match(/../g).map((channel) => parseInt(channel, 16));
const luminance = (hex) => channels(hex).map((channel) => channel / 255).map((value) => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4).reduce((sum, value, index) => sum + value * [.2126, .7152, .0722][index], 0);
const contrast = (a, b) => (Math.max(luminance(a), luminance(b)) + .05) / (Math.min(luminance(a), luminance(b)) + .05);

function tokens(source, dark) {
  const values = {};
  for (const [, selector, body] of source.matchAll(/(?:^|\n):root([^{}]*)\{([^}]+)\}/g)) {
    if (selector.includes("dark") && !dark) continue;
    for (const [, name, value] of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) values[name] = value.trim();
  }
  return values;
}

function resolve(value, values) {
  const variable = value.match(/^var\((--[\w-]+)\)$/);
  if (variable) return resolve(values[variable[1]], values);
  const mix = value.match(/^color-mix\(in srgb, (.+) (\d+(?:\.\d+)?)%, (.+)\)$/);
  if (!mix) return value;
  const first = channels(resolve(mix[1], values)), second = channels(resolve(mix[3], values));
  const ratio = Number(mix[2]) / 100;
  return "#" + first.map((channel, index) => Math.round(channel * ratio + second[index] * (1 - ratio)).toString(16).padStart(2, "0")).join("");
}

it("uses the same five Oya base colors in the desktop and public website", () => {
  for (const source of [read("./design/tokens.css"), website]) expect([...new Set(source.match(/#[\da-f]{6}\b/gi))]).toEqual(BASES);
});

for (const dark of [false, true]) {
  const values = { ...tokens(styles, dark), ...tokens(workspace, dark), ...tokens(ide, dark) };
  const color = (name) => resolve(values[name], values);
  it(`${dark ? "dark" : "light"} preview annotation numbers use contrasting foreground and fill`, () => {
    const pin = styles.match(/\.pin-n\s*\{([^}]+)\}/)[1];
    const background = resolve(pin.match(/\bbackground:\s*([^;]+)/)[1], values);
    const foreground = resolve(pin.match(/(?:^|;)\s*color:\s*([^;]+)/)[1], values);
    expect(contrast(foreground, background)).toBeGreaterThanOrEqual(4.5);
  });
  it(`${dark ? "dark" : "light"} window and independent terminal colors share readable Oya roles`, () => {
    const terminal = terminalPalette(dark);
    expect(color("--well")).toBe(terminal.background);
    expect(color("--surface")).toBe(terminal.surface);
    expect(color("--text")).toBe(terminal.foreground);
    expect(color("--add")).toBe(terminal.green);
    expect(color("--del")).toBe(terminal.red);
    expect(color("--warn")).toBe(terminal.yellow);
    expect(terminal.ansi).toHaveLength(16);
  });
  it(`${dark ? "dark" : "light"} text, status and focus roles remain visible on workspace surfaces`, () => {
    for (const surface of ["--bg", "--surface", "--raised", "--well"]) {
      for (const text of ["--text", "--dim", "--accent", "--add", "--del", "--warn"]) {
        expect(contrast(color(text), color(surface)), `${text} on ${surface}`).toBeGreaterThanOrEqual(4.5);
      }
      expect(contrast(color("--control-edge"), color(surface))).toBeGreaterThanOrEqual(3);
    }
    expect(contrast(color("--on-accent"), color("--accent-fill"))).toBeGreaterThanOrEqual(4.5);
    expect(color("--accent-fill")).toBe("#cdf01e");
    expect(color("--on-accent")).toBe("#1a1a1a");
  });
}
