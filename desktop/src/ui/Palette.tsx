import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";

export interface Command {
  id: string;
  title: string;
  detail?: string;
  keys?: string;
  run: () => void;
}

/// Subsequence match with a bonus for word starts and runs: `nlb` finds "New lane on its own
/// branch". Ported from the Swift palette's `Fuzzy`.
export function score(query: string, text: string): { score: number; at: number[] } | null {
  if (!query) return { score: 0, at: [] };
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  const at: number[] = [];
  let s = 0;
  let qi = 0;
  let last = -2;
  for (let i = 0; i < t.length && qi < q.length; i++) {
    if (t[i] !== q[qi]) continue;
    const start = i === 0 || /[\s/·:_-]/.test(t[i - 1]);
    s += 1 + (start ? 4 : 0) + (i === last + 1 ? 2 : 0);
    at.push(i);
    last = i;
    qi++;
  }
  return qi === q.length ? { score: s - t.length * 0.01, at } : null;
}

function Highlight({ text, at }: { text: string; at: number[] }) {
  if (!at.length) return <>{text}</>;
  const set = new Set(at);
  return (
    <>
      {[...text].map((ch, i) => (set.has(i) ? <b key={i}>{ch}</b> : <span key={i}>{ch}</span>))}
    </>
  );
}

export function Palette({ commands, close }: { commands: Command[]; close: () => void }) {
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);
  const rows = useMemo(
    () =>
      commands
        .map((c) => ({ c, m: score(query, c.title) ?? (query ? score(query, `${c.title} ${c.detail ?? ""}`) : null) }))
        .filter((r): r is { c: Command; m: { score: number; at: number[] } } => !!r.m)
        .sort((a, b) => b.m.score - a.m.score)
        .slice(0, 50),
    [commands, query],
  );
  useEffect(() => setIndex(0), [query]);
  function key(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Escape") close();
    else if (e.key === "ArrowDown") {
      e.preventDefault();
      setIndex((i) => Math.min(rows.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => Math.max(0, i - 1));
    }
    else if (e.key === "Enter" && rows[index]) {
      e.preventDefault();
      close();
      rows[index].c.run();
    }
  }
  return (
    <>
      <div className="scrim" onClick={close} />
      <div className="palette" role="dialog" aria-label="Command palette">
        <input ref={input} value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={key} placeholder="Go to a lane, resume a session, or run an action" aria-label="Search commands" />
        {rows.length === 0 ? (
          <div className="none">No matches — try a lane, a session or an action.</div>
        ) : (
          <ul role="listbox">
            {rows.map((r, i) => (
              <li
                key={r.c.id}
                role="option"
                aria-selected={i === index}
                onMouseEnter={() => setIndex(i)}
                onClick={() => {
                  close();
                  r.c.run();
                }}
              >
                <span className="title">
                  <Highlight text={r.c.title} at={r.m.at} />
                </span>
                {r.c.detail && <span className="detail">{r.c.detail}</span>}
                {r.c.keys && <span className="keys">{r.c.keys}</span>}
              </li>
            ))}
          </ul>
        )}
      </div>
    </>
  );
}
