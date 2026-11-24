// Native agent and shell terminals retain their process and input ownership across lane switches.
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { onQuickFailure, quickFailure, quickWaiting, registerShell, shellFailed } from "../quick-commands";
import { canAutofocusTerminal, terminalConnectionFocus } from "../terminal-focus";
import { type Endpoint } from "../api";
import { AGENT_NAME, useStore } from "../store";
import { register } from "../typers";
import { nextFollow, PERSON_SCROLL_MS } from "../terminal-follow";
import { consumeChatFocus } from "../chat-focus";
import { isKeel } from "../keys";
import type { SearchAddon } from "@xterm/addon-search";
import { TerminalFind, useFindKey } from "./TerminalFind";
import { onTerminalFont, terminalFont } from "../zoom";
import { onTerminalTheme, terminalDark } from "../terminal-theme";
import { terminalXtermTheme as theme } from "../terminal-palette";
import { SubagentsPane } from "./SubagentsPane";
import { TerminalFileSurface } from "./TerminalFileFeedback";

/// A terminal over the daemon's PTY: the lane's agent itself (`agent`), or a shell in its checkout.
///
/// xterm.js rather than a renderer of our own, for the reason the Swift app used SwiftTerm: a
/// terminal is mostly escape sequences and input methods, and a home-made one gets both wrong.
/// An agent terminal stays mounted while its lane is off screen (`hidden`): closing the socket
/// ends the process, and switching lanes must never stop a turn.
/// `term::ELSEWHERE`: the close code for "this conversation is open in another process".
const ELSEWHERE = 4001;

/// The wheel scrolls the history. A TUI that turns on mouse reporting (Claude Code does) gets
/// every wheel event instead, and the scrollback above it could no longer be reached. In the
/// normal buffer the history is Oya Factory's to scroll; in the alternate screen there is none, and the
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

/// Hold the bottom of the output unless the person scrolled away (`terminal-follow.ts`). The
/// How every terminal starts: the theme and font of the moment, and room for a long scrollback.
function xtermOptions() {
  return {
    // Readable whatever the program asks for: xterm lifts any colour that would fall under
    // this contrast against its background — Claude Code's dim text included.
    minimumContrastRatio: 4.5,
    fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--mono"),
    fontSize: terminalFont(),
    lineHeight: 1.2,
    cursorBlink: true,
    allowProposedApi: true,
    scrollback: 5_000,
    theme: theme(terminalDark()),
  };
}

/// returned function stops watching; its `settle` runs after each write.
function followBottom(term: import("@xterm/xterm").Terminal, host: HTMLElement) {
  let following = true;
  let touched = -Infinity;
  let held = false;
  const touch = () => { touched = performance.now(); };
  const press = () => { held = true; touch(); };
  const release = () => { held = false; touch(); };
  const settle = (byPerson = false) => {
    const buffer = term.buffer.active;
    const next = nextFollow(following, buffer.viewportY >= buffer.baseY, byPerson);
    following = next.following;
    if (next.toBottom) term.scrollToBottom();
  };
  const scrolled = () => settle(held || performance.now() - touched < PERSON_SCROLL_MS);
  const viewport = host.querySelector(".xterm-viewport");
  host.addEventListener("wheel", touch, { capture: true, passive: true });
  host.addEventListener("keydown", touch, true);
  host.addEventListener("pointerdown", press, true);
  window.addEventListener("pointerup", release, true);
  viewport?.addEventListener("scroll", scrolled);
  const stop = () => {
    host.removeEventListener("wheel", touch, true);
    host.removeEventListener("keydown", touch, true);
    host.removeEventListener("pointerdown", press, true);
    window.removeEventListener("pointerup", release, true);
    viewport?.removeEventListener("scroll", scrolled);
  };
  return Object.assign(stop, { settle: () => settle() });
}

function setElsewhere(lane: string, elsewhere: boolean) {
  useStore.setState((s) => (s.lanes[lane] ? { lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], elsewhere } } } : s));
}

// Visibility belongs to a socket registration, so an old cleanup cannot invalidate its replacement.
function providerInput(lane: string, ws: WebSocket, term: import("@xterm/xterm").Terminal, visible: () => boolean, alive: () => boolean) {
  const encoder = new TextEncoder();
  const connection = register(lane, (text) => {
    if (!alive() || ws.readyState !== WebSocket.OPEN) return false;
    ws.send(encoder.encode(text)); return true;
  }, { visible: visible(), focus: () => term.focus() });
  const refresh = () => connection.visible(visible());
  const unwatch = useStore.subscribe((state, before) => { if (state.active !== before.active) refresh(); });
  return { refresh, close: () => { unwatch(); connection(); } };
}

/** Connection readiness applies focus policy after layout has chosen the visible destination. */
function terminalReady(options: { opening: ReturnType<typeof terminalConnectionFocus>; term: { focus(): void }; hidden: { current: boolean }; ready(): void; resize(): void; agent: boolean; lane: string }) {
  const focus = options.opening.allowed(document.activeElement);
  options.opening.dispose(); options.ready(); options.resize();
  if (!options.hidden.current && focus) options.term.focus();
  if (options.agent && !options.hidden.current) consumeChatFocus(options.lane, options.term);
}

export function Terminal({ lane, agent = false, hidden = false, onExit }: { lane: string; agent?: boolean; hidden?: boolean; onExit?: () => void }) {
  const host = useRef<HTMLDivElement>(null);
  const fitRef = useRef<() => void>(() => {});
  const visibilityRef = useRef<() => void>(() => {});
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
  const quickError = useSyncExternalStore(onQuickFailure, () => quickFailure(lane));
  const quickWaits = useSyncExternalStore(onQuickFailure, () => quickWaiting(lane));
  const [search, setSearch] = useState<SearchAddon>();
  const [finding, setFinding] = useState(false);
  const [findToken, setFindToken] = useState(0);
  useFindKey(host, () => { setFinding(true); setFindToken((t) => t + 1); }, elsewhere);
  // A quick command waiting on a shell that exited starts it again rather than timing out.
  useEffect(() => { if (!agent && quickWaits && state === "closed") setGeneration((g) => g + 1); }, [agent, quickWaits, state]);

  useEffect(() => {
    if (!host.current || elsewhere) return;
    let disposed = false;
    const openingFocus = terminalConnectionFocus(document.activeElement, !agent);
    let cleanup = () => openingFocus.dispose();
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
        openingFocus.dispose();
        if (!ep && !disposed) { setState("closed"); if (!agent) shellFailed(lane, "Oya Factory could not reach this project."); }
        return;
      }
      const [{ Terminal: XTerm }, { FitAddon }, { SearchAddon }] = await Promise.all([import("@xterm/xterm"), import("@xterm/addon-fit"), import("@xterm/addon-search")]);
      await import("@xterm/xterm/css/xterm.css");
      if (disposed || !host.current) return;
      setBg(theme(terminalDark()).background);
      const term = new XTerm(xtermOptions());
      const fit = new FitAddon();
      term.loadAddon(fit);
      const search = new SearchAddon(); term.loadAddon(search); setSearch(search);
      // Oya Factory's own chords go up to the window; every other key, Esc and ⌃C included, is the CLI's.
      term.attachCustomKeyEventHandler((e) => !isKeel(e));
      wheelScrollsHistory(term);
      term.open(host.current);
      const unfollow = followBottom(term, host.current);
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
        params.set("theme", terminalDark() ? "dark" : "light");
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
      ws.onopen = () => terminalReady({ opening: openingFocus, term, hidden: hiddenRef, ready: () => setState("open"), resize, agent, lane });
      ws.onmessage = (e) => {
        // A text frame is the tab title, never output: writing it to the screen printed `zsh`
        // into the shell.
        if (typeof e.data === "string") setTitle(e.data);
        else term.write(new Uint8Array(e.data as ArrayBuffer), unfollow.settle);
      };
      ws.onclose = (e) => {
        openingFocus.dispose();
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
        if (!agent) { unregister = registerShell(lane, (text) => ws.readyState === WebSocket.OPEN && (ws.send(encoder.encode(text)), true)); return; }
        const connection = providerInput(lane, ws, term, () => !hiddenRef.current && useStore.getState().active === lane, () => !disposed);
        visibilityRef.current = connection.refresh;
        unregister = () => { connection.close(); if (visibilityRef.current === connection.refresh) visibilityRef.current = () => {}; };
        useStore.getState().connected(lane);
      });
      ws.addEventListener("close", () => { unregister(); if (!agent) shellFailed(lane, "The shell closed before it opened."); });
      const observer = new ResizeObserver(() => resize());
      observer.observe(host.current);
      // A new size is a new cell grid: refit, so the CLI is told its new rows and columns.
      const unfont = onTerminalFont((size) => {
        term.options.fontSize = size;
        resize();
      });
      cleanup = () => {
        openingFocus.dispose();
        unfont();
        unfollow();
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
      setSearch(undefined);
      setFinding(false);
      cleanup();
    };
  }, [lane, agent, generation, elsewhere]);

  const onExitRef = useRef(onExit);
  onExitRef.current = onExit;
  const takeoverRef = useRef(false);
  const hiddenRef = useRef(hidden);
  hiddenRef.current = hidden;
  useEffect(() => {
    visibilityRef.current();
    if (hidden) {
      setFinding(false);
      // A hidden xterm that keeps focus still takes keystrokes — into a lane nobody can see.
      if (host.current?.contains(document.activeElement)) (document.activeElement as HTMLElement).blur();
      return;
    }
    requestAnimationFrame(() => {
      fitRef.current();
      // Typing follows the lane on screen — unless focus is in the rail, the palette or a dialog.
      if (canAutofocusTerminal(document.activeElement)) host.current?.querySelector("textarea")?.focus();
    });
  }, [hidden]);

  const name = agent ? (kind === "claude" ? "Claude Code" : AGENT_NAME[kind]) : "Shell";
  if (elsewhere)
    return (
      <TerminalFileSurface lane={lane} agent={agent} hidden={hidden}>
        <div className="panel-empty" style={{ margin: "auto" }}>
          <h3>This conversation is running in another terminal</h3>
          <p>Oya Factory is following it: each turn appears on the right as it happens. Starting it here too would be two agents writing one conversation.</p>
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
      </TerminalFileSurface>
    );
  return (
    <TerminalFileSurface lane={lane} agent={agent} hidden={hidden}>
      <SubagentsPane lane={lane} hidden={hidden} enabled={agent}>
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
      {!agent && quickError && <div className="terminal-state small"><span className="error">{quickError}</span></div>}
      {!agent && title && state === "open" && <div className="panel-head small muted">{title}</div>}
      <div className="terminal-frame">
        {finding && search && state === "open" && <TerminalFind search={search} selectToken={findToken} onClose={() => setFinding(false)} focusTerminal={() => host.current?.querySelector("textarea")?.focus()} />}
        <div className="terminal-host" ref={host} style={bg ? { background: bg } : undefined} />
      </div>
      </SubagentsPane>
    </TerminalFileSurface>
  );
}
