import { useEffect, useRef, useState } from "react";
import { type Endpoint } from "../api";
import { AGENT_NAME, useStore } from "../store";
import { register } from "../typers";
import { isKeel } from "../keys";
import { onTerminalFont, terminalFont } from "../zoom";
import { onTerminalTheme, terminalDark } from "../terminal-theme";

/// A terminal over the daemon's PTY: the lane's agent itself (`agent`), or a shell in its checkout.
///
/// xterm.js rather than a renderer of our own, for the reason the Swift app used SwiftTerm: a
/// terminal is mostly escape sequences and input methods, and a home-made one gets both wrong.
/// An agent terminal stays mounted while its lane is off screen (`hidden`): closing the socket
/// ends the process, and switching lanes must never stop a turn.
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
    ansi: ["#1c1c1a", "#c0322a", "#16774a", "#9a5200", "#2f5fb3", "#8a3fa0", "#1b7a86", "#62625a", "#45453e", "#d5473e", "#157a13", "#b06a12", "#3d6fc7", "#9d52b3", "#238e9b", "#1c1c1a"],
  },
  dark: {
    background: "#141410",
    foreground: "#f4f3ef",
    cursor: "#39ed35",
    selection: "rgba(57, 237, 53, 0.25)",
    ansi: ["#2c2c26", "#ff7b72", "#5cd39a", "#f2b544", "#79a8ff", "#d199ff", "#6fd3dd", "#babab0", "#62625a", "#ff9a92", "#7fe0b0", "#f7c96b", "#9cbfff", "#ddb3ff", "#93e2ea", "#f4f3ef"],
  },
};

function theme(dark: boolean) {
  const p = dark ? PALETTES.dark : PALETTES.light;
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

/// `term::ELSEWHERE`: the close code for "this conversation is open in another process".
const ELSEWHERE = 4001;

/// The wheel scrolls the history. A TUI that turns on mouse reporting (Claude Code does) gets
/// every wheel event instead, and the scrollback above it could no longer be reached. In the
/// normal buffer the history is Keel's to scroll; in the alternate screen there is none, and the
/// wheel is the program's.
function wheelScrollsHistory(term: import("@xterm/xterm").Terminal) {
  let wheel = 0;
  term.attachCustomWheelEventHandler((e) => {
    if (term.buffer.active.type !== "normal") return true;
    const line = (term.options.fontSize ?? 13) * (term.options.lineHeight ?? 1.2);
    wheel += e.deltaMode === 1 ? e.deltaY * line : e.deltaMode === 2 ? e.deltaY * line * term.rows : e.deltaY;
    const lines = Math.trunc(wheel / line);
    if (lines) {
      term.scrollLines(lines);
      wheel -= lines * line;
    }
    e.preventDefault();
    return false;
  });
}

function setElsewhere(lane: string, elsewhere: boolean) {
  useStore.setState((s) => (s.lanes[lane] ? { lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], elsewhere } } } : s));
}

export function Terminal({ lane, agent = false, hidden = false, onExit }: { lane: string; agent?: boolean; hidden?: boolean; onExit?: () => void }) {
  const host = useRef<HTMLDivElement>(null);
  const fitRef = useRef<() => void>(() => {});
  const [title, setTitle] = useState("");
  const [state, setState] = useState<"starting" | "open" | "closed">("starting");
  const [generation, setGeneration] = useState(0);
  const [error, setError] = useState<string>();
  const kind = useStore((s) => s.lanes[lane]?.agent ?? "claude");
  const kindRef = useRef(kind);
  kindRef.current = kind;
  const [bg, setBg] = useState<string>();
  const wtName = useStore((s) => s.lanes[lane]?.wt);
  const elsewhere = useStore((s) => agent && !!s.lanes[lane]?.elsewhere);

  useEffect(() => {
    if (!host.current || elsewhere) return;
    let disposed = false;
    let cleanup = () => {};
    setState("starting");
    setError(undefined);
    void (async () => {
      const store = useStore.getState();
      const l = store.lanes[lane];
      if (!l) return;
      let ep: Endpoint | undefined;
      let wt = l.wt;
      try {
        const ready = await store.prepare(lane);
        ep = ready?.ep;
        wt = ready?.wt ?? wt;
      } catch (e) {
        setError(String(e));
      }
      if (!ep || disposed || !host.current) {
        if (!ep && !disposed) setState("closed");
        return;
      }
      const [{ Terminal: XTerm }, { FitAddon }] = await Promise.all([import("@xterm/xterm"), import("@xterm/addon-fit")]);
      await import("@xterm/xterm/css/xterm.css");
      if (disposed || !host.current) return;
      const css = getComputedStyle(document.documentElement);
      const dark = terminalDark();
      setBg(theme(dark).background);
      const term = new XTerm({
        // Readable whatever the program asks for: xterm lifts any colour that would fall under
        // this contrast against its background — Claude Code's dim text included.
        minimumContrastRatio: 4.5,
        fontFamily: css.getPropertyValue("--mono"),
        fontSize: terminalFont(),
        lineHeight: 1.2,
        cursorBlink: true,
        allowProposedApi: true,
        scrollback: 5_000,
        theme: theme(dark),
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      // Keel's own chords go up to the window; every other key, Esc and ⌃C included, is the CLI's.
      term.attachCustomKeyEventHandler((e) => !isKeel(e));
      wheelScrollsHistory(term);
      term.open(host.current);
      // Every terminal follows the choice, live. A running agent keeps the colours it started
      // with until it next starts — Claude Code reads its theme once, at launch.
      const retheme = () => {
        term.options.theme = theme(terminalDark());
        setBg(theme(terminalDark()).background);
      };
      const untheme = onTerminalTheme(retheme);
      const current = useStore.getState().lanes[lane];
      const params = new URLSearchParams();
      if (wt) params.set("wt", wt);
      if (agent && current?.session) {
        params.set("agent", current.agent);
        params.set("session", current.session);
        params.set("lane", lane);
        // Codex and Antigravity name their own conversations; until the daemon has found the id
        // on disk (`terminal.session`), the lane starts fresh rather than resuming nothing.
        if (current.known) params.set("resume", "true");
        if (takeoverRef.current) params.set("takeover", "true");
        // Claude Code is started in the terminal's theme; Codex and agy ask the terminal itself.
        params.set("theme", dark ? "dark" : "light");
      }
      // The token goes as a subprotocol: a page's WebSocket cannot set headers, and a URL is
      // somewhere it would be logged.
      const ws = new WebSocket(`ws://127.0.0.1:${ep.port}/api/term/ws?${params}`, ["keel", `keel.${ep.token}`]);
      ws.binaryType = "arraybuffer";
      const encoder = new TextEncoder();
      const resize = () => {
        if (!host.current?.offsetWidth) return; // hidden: nothing to measure
        fit.fit();
        if (ws.readyState === WebSocket.OPEN) ws.send(`\u0000${term.cols},${term.rows}`);
      };
      fitRef.current = resize;
      ws.onopen = () => {
        setState("open");
        resize();
        if (!hiddenRef.current) term.focus();
      };
      ws.onmessage = (e) => {
        // A text frame is the tab title, never output: writing it to the screen printed `zsh`
        // into the shell.
        if (typeof e.data === "string") setTitle(e.data);
        else term.write(new Uint8Array(e.data as ArrayBuffer));
      };
      ws.onclose = (e) => {
        // The daemon found this conversation open in another process: follow it, don't fork it.
        if (e.code === ELSEWHERE) {
          if (!disposed) setElsewhere(lane, true);
          return;
        }
        setState("closed");
        // A shell you typed `exit` into is done; an agent's exit row offers "Start again".
        if (!disposed) onExitRef.current?.();
      };
      const input = term.onData((d) => ws.readyState === WebSocket.OPEN && ws.send(encoder.encode(d)));
      // Registered while connected and only then: a prompt typed into a socket that is still
      // opening, or has closed, was lost — after its job had already been marked delivered.
      let unregister = () => {};
      ws.addEventListener("open", () => {
        if (!agent) return;
        unregister = register(lane, (text) => ws.send(encoder.encode(text)));
        useStore.getState().connected(lane);
      });
      ws.addEventListener("close", () => unregister());
      const observer = new ResizeObserver(() => resize());
      observer.observe(host.current);
      // A new size is a new cell grid: refit, so the CLI is told its new rows and columns.
      const unfont = onTerminalFont((size) => {
        term.options.fontSize = size;
        resize();
      });
      cleanup = () => {
        unfont();
        untheme();
        unregister();
        observer.disconnect();
        input.dispose();
        ws.close();
        term.dispose();
      };
    })();
    return () => {
      disposed = true;
      cleanup();
    };
  }, [lane, agent, generation, elsewhere]);

  const onExitRef = useRef(onExit);
  onExitRef.current = onExit;
  const takeoverRef = useRef(false);
  const hiddenRef = useRef(hidden);
  hiddenRef.current = hidden;
  useEffect(() => {
    if (hidden) {
      // A hidden xterm that keeps focus still takes keystrokes — into a lane nobody can see.
      if (host.current?.contains(document.activeElement)) (document.activeElement as HTMLElement).blur();
      return;
    }
    requestAnimationFrame(() => {
      fitRef.current();
      // Typing follows the lane on screen — unless focus is in the rail, the palette or a dialog.
      if (!document.activeElement?.closest(".sidebar, .palette, [role=dialog], .strip, .side-panel, input, textarea:not(.xterm-helper-textarea)")) host.current?.querySelector("textarea")?.focus();
    });
  }, [hidden]);

  const name = agent ? (kind === "claude" ? "Claude Code" : AGENT_NAME[kind]) : "Shell";
  if (elsewhere)
    return (
      <div className={`terminal ${hidden ? "hidden" : ""}`} aria-hidden={hidden}>
        <div className="panel-empty" style={{ margin: "auto" }}>
          <h3>This conversation is running in another terminal</h3>
          <p>Keel is following it: each turn appears on the right as it happens. Starting it here too would be two agents writing one conversation.</p>
          <button
            className="primary"
            onClick={() => {
              takeoverRef.current = true;
              setElsewhere(lane, false);
            }}
            title="Resume it here. Stop the other one first, or the two will interleave."
          >
            Take over here
          </button>
        </div>
      </div>
    );
  return (
    <div className={`terminal ${hidden ? "hidden" : ""}`} aria-hidden={hidden}>
      {state !== "open" && (
        <div className="terminal-state small">
          {error ? (
            <span className="error">{error}</span>
          ) : state === "starting" ? (
            <span className="muted">Starting {name}{agent ? (wtName ? ` in keel/${wtName}` : " in the project's working tree") : ""}…</span>
          ) : (
            <>
              <span className="muted">{name} exited.</span>{" "}
              <button className="link" onClick={() => setGeneration((g) => g + 1)}>
                Start again
              </button>
              {agent && !wtName && (
                <button
                  className="link"
                  onClick={() => {
                    useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], isolated: true } } }));
                    setGeneration((g) => g + 1);
                  }}
                  title="If another lane is writing this tree, give this one a branch and checkout of its own"
                >
                  Give it its own branch
                </button>
              )}
            </>
          )}
        </div>
      )}
      {!agent && title && state === "open" && <div className="panel-head small muted">{title}</div>}
      <div className="terminal-host" ref={host} style={bg ? { background: bg } : undefined} />
    </div>
  );
}
