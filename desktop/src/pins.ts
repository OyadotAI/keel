// What a pin is, and the prompt a set of them becomes. Pure, so it can be tested on its own.

export interface Source {
  kind: "inspector" | "react" | "svelte" | "vue" | "component" | "testid";
  file?: string;
  line?: number;
  name?: string;
}
export interface Picked {
  selector: string;
  exact: boolean;
  tag: string;
  text: string;
  rect: { x: number; y: number; width: number; height: number };
  sources: Source[];
  styles: Record<string, string>;
  print: string;
  url: string;
  frame?: string;
}
export type Verdict = "changed" | "unchanged" | "gone" | "unchecked";
export interface Pin extends Picked {
  id: string;
  note: string;
  sent?: boolean;
  verdict?: Verdict;
}
/// A field from the page, made safe to put in a prompt: one line, no control characters, capped.
/// Everything a pin carries came from the dev server's page — the project's dependencies' code —
/// and it is typed into the agent's terminal.
export const flat = (s: unknown, max = 200) =>
  String(s ?? "")
    // eslint-disable-next-line no-control-regex -- stripping control characters is the point.
    .replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, max);

/// The prompt for the pins not yet sent: what to change, the element, and where it most likely
/// comes from — ranked, with its kind, so a guess reads as a guess.
export function prompt(pins: Pin[]): string {
  const lines = ["Design change in the running app (" + (flat(pins[0]?.url, 300) || "the preview") + "):", ""];
  pins.forEach((p, i) => {
    lines.push(`${i + 1}. ${flat(p.note, 1000) || "(no note — ask what to change)"}`);
    const text = p.text ? ` — "${flat(p.text, 80)}"` : "";
    lines.push(`   Element: <${flat(p.tag, 30)}> \`${flat(p.selector, 300)}\`${p.exact ? "" : " (not unique — the closest selector found)"}${text}`);
    const where = (Array.isArray(p.sources) ? p.sources.slice(0, 5) : []).map((s) =>
      s.file ? `${flat(s.file, 200)}${Number.isFinite(s.line) ? `:${s.line}` : ""} (${flat(s.kind, 20)})` : `${flat(s.kind, 20)} ${flat(s.name, 80)}`,
    );
    lines.push(`   Likely source: ${where.length ? where.join("; ") : "no hint — find it from the selector and text"}`);
  });
  lines.push("", "Edit the source that renders each element. Do not copy or fork a component to change one instance of it unless the note asks for that.");
  return lines.join("\n");
}

