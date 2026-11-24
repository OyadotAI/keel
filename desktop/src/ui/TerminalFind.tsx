// Find in a terminal's scrollback: a bar floated over the terminal so the grid never refits and the
// agent's screen does not redraw. The addon is loaded in the terminal's own lazy chunk.
import { useEffect, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import type { SearchAddon } from "@xterm/addon-search";
import { ideCommand } from "../ide-keys";
import { MAC, matches } from "../keys";
import { Icon } from "./icons";
import { terminalFindDecorations } from "../design/terminal";

/// ⌘F (Ctrl+Shift+F on Windows, or the editor's Find chord) inside a terminal opens its find bar.
/// Caught on the host, before the window's handler, so the editor's Find keeps it everywhere else.
export function useFindKey(host: RefObject<HTMLElement | null>, open: () => void, remount?: unknown) {
  const openRef = useRef(open);
  openRef.current = open;
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const key = (e: globalThis.KeyboardEvent) => {
      if (!(matches(e, { key: "f" }) || ideCommand(e, MAC) === "find")) return;
      e.preventDefault();
      e.stopPropagation();
      openRef.current();
    };
    el.addEventListener("keydown", key);
    return () => el.removeEventListener("keydown", key);
    // `remount`: the host element is replaced when the terminal comes back from another view.
  }, [host, remount]);
}


/// The addon stops counting past this; the count then reads "1000+".
const COUNT_CAP = 1000;

/// What the count says for the addon's latest result.
export function findCount(query: string, r?: { resultIndex: number; resultCount: number }): string {
  if (!query || !r) return "";
  if (r.resultCount === 0) return "No matches";
  if (r.resultCount >= COUNT_CAP) return `${COUNT_CAP}+`;
  if (r.resultIndex < 0) return `${r.resultCount} matches`;
  return `${r.resultIndex + 1} of ${r.resultCount}`;
}

/// The bar. `focusTerminal` puts typing back where it was when the bar closes.
export function TerminalFind({ search, onClose, focusTerminal, selectToken }: { search: SearchAddon; onClose: () => void; focusTerminal: () => void; selectToken: number }) {
  const input = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<{ resultIndex: number; resultCount: number }>();
  useEffect(() => {
    const sub = search.onDidChangeResults((r) => setResult(r));
    return () => {
      sub.dispose();
      search.clearDecorations();
    };
  }, [search]);
  // ⌘F again while open selects what is typed, like every find bar.
  useEffect(() => input.current?.select(), [selectToken]);
  const go = (forward: boolean, q = query) => {
    if (!q) return search.clearDecorations();
    const opts = { decorations: terminalFindDecorations(), incremental: false };
    if (forward) search.findNext(q, opts);
    else search.findPrevious(q, opts);
  };
  const close = () => {
    onClose();
    focusTerminal();
  };
  function keys(e: KeyboardEvent<HTMLInputElement>) {
    // Nothing typed here reaches the terminal underneath.
    e.stopPropagation();
    if (matches(e.nativeEvent, { key: "f" }) || ideCommand(e.nativeEvent, MAC) === "find") input.current?.select();
    else if (e.key === "Escape") close();
    else if (e.key === "Enter") go(!e.shiftKey);
    else return;
    e.preventDefault();
  }
  const count = findCount(query, result);
  return (
    <div className="terminal-find" role="search">
      <input
        ref={input}
        autoFocus
        placeholder="Find"
        aria-label="Find in terminal"
        value={query}
        onKeyDown={keys}
        onChange={(e) => {
          setQuery(e.target.value);
          if (!e.target.value) setResult(undefined);
          go(true, e.target.value);
        }}
      />
      <span className={`terminal-find-count${count === "No matches" ? " dim" : ""}`} aria-live="polite">{count}</span>
      <button className="ghost" aria-label="Previous match" title="Previous match (⇧Enter)" disabled={!query} onClick={() => go(false)}><Icon name="chevron-down" size={13} style={{ transform: "rotate(180deg)" }} /></button>
      <button className="ghost" aria-label="Next match" title="Next match (Enter)" disabled={!query} onClick={() => go(true)}><Icon name="chevron-down" size={13} /></button>
      <button className="ghost" aria-label="Close (Esc)" title="Close (Esc)" onClick={close}><Icon name="x" size={12} /></button>
    </div>
  );
}
