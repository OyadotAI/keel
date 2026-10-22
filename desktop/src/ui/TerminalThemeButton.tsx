// The terminal's colours, chosen where the terminal is: in the header of a lane showing one. One
// choice for every agent terminal (`terminal-theme`), because Claude Code, Codex and agy are each
// drawn for the background they are given.

import { useState } from "react";
import { useStore } from "../store";
import { setTerminalTheme, useTerminalTheme, type TerminalTheme } from "../terminal-theme";
import { Floating } from "./Menu";
import { Icon } from "./icons";

const CHOICES = [
  ["window", "Match the window", "monitor"],
  ["light", "Light", "sun"],
  ["dark", "Dark", "moon"],
] as const satisfies readonly (readonly [TerminalTheme, string, string])[];

export function TerminalThemeButton({ lane }: { lane: string }) {
  const terminal = useStore((s) => s.lanes[lane]?.interface === "terminal");
  const mode = useTerminalTheme();
  const [menu, setMenu] = useState<DOMRect | null>(null);
  if (!terminal) return null;
  const icon = CHOICES.find(([value]) => value === mode)?.[2] ?? "monitor";
  return (
    <>
      <button className="ghost" aria-label="Terminal colours" title="Terminal colours" aria-haspopup="menu" onClick={(e) => setMenu(e.currentTarget.getBoundingClientRect())}>
        <Icon name={icon} />
      </button>
      {menu && (
        <Floating anchor={menu} onClose={() => setMenu(null)} width={260}>
          {CHOICES.map(([value, title, glyph]) => (
            <button key={value} role="menuitemradio" aria-checked={mode === value} onClick={() => (setMenu(null), setTerminalTheme(value))}>
              <Icon name={glyph} size={13} /> {title}{mode === value ? " ✓" : ""}
            </button>
          ))}
          <div className="sep" />
          <p className="small muted" style={{ margin: "4px 10px 6px" }}>A running agent takes the new colours the next time it starts.</p>
        </Floating>
      )}
    </>
  );
}
