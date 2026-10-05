import { memo, useEffect, useMemo, useState } from "react";
import { get, post } from "../api";
import { prepare, type DiffResponse, type Prepared } from "../git";
import { JOB_PREFIX, useStore } from "../store";
import type { Call, Turn, Usage } from "../reduce";
import { Icon, type IconName } from "./icons";
import { InlineDiff } from "./InlineDiff";

/// A turn as a card: what it was asked, what it did to the repository, and how the checks went.
/// Not what it said — the terminal shows that, and two panes printing the same prose made both
/// unreadable in the Swift app.

type State = "busy" | "waiting" | "checking" | "committed" | "passed" | "failed" | "unchecked" | "agent-failed" | "interrupted" | "replayed" | "quiet";

/// A state is a word and a tone. The tone colours one 7px dot and nothing else: a feed where every
/// entry shouts its state in a coloured border is a feed where nothing stands out.
const LOOK: Record<State, { word: string; tone: "live" | "warn" | "ok" | "bad" | "idle" }> = {
  busy: { word: "Running", tone: "live" },
  waiting: { word: "Needs you", tone: "warn" },
  checking: { word: "Checking", tone: "live" },
  committed: { word: "Committed", tone: "ok" },
  passed: { word: "Passed", tone: "ok" },
  failed: { word: "Checks failed", tone: "bad" },
  unchecked: { word: "Not checked", tone: "idle" },
  "agent-failed": { word: "Did not finish", tone: "bad" },
  interrupted: { word: "Stopped", tone: "idle" },
  replayed: { word: "Done", tone: "idle" },
  quiet: { word: "Done", tone: "idle" },
};

export function stateOf(t: Turn, waiting: boolean): State {
  if (!t.closed) return waiting ? "waiting" : "busy";
  if (t.gate?.status === "running") return "checking";
  if (t.gate?.status === "failed") return "failed";
  if (t.closed === "interrupted" || t.closed === "stopped") return "interrupted";
  if (t.failures.some((f) => !f.history) && !t.gate) return "agent-failed";
  if (t.commit) return "committed";
  if (t.gate?.status === "passed") return "passed";
  if (t.gate?.status === "none") return "unchecked";
  if (!t.snapshot && !t.gate && !t.ended) return t.files.length || Object.keys(t.calls).length ? "replayed" : "quiet";
  return t.files.length ? "unchecked" : "quiet";
}

/// What a prompt says, for one line: its text, or what it was when it had none.
export function promptLine(prompt: string | null | undefined): string {
  const text = (prompt ?? "").replace(/\[Image #\d+\]/g, "").trim();
  if (!prompt) return "Earlier in the conversation";
  return text || "An image";
}

const time = (iso?: string) => (iso ? new Date(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : "");

/// One quiet turn — it answered and changed nothing — as a single line in its group.
export function QuietLine({ lane, index }: { lane: string; index: number }) {
  const t = useStore(pick(lane, index, (t) => t));
  if (!t) return null;
  return (
    <div className="quiet-line" title={t.prompt ?? undefined}>
      <span className="quiet-text">{promptLine(t.prompt)}</span>
      <span className="num faint">{time(t.startedAt)}</span>
    </div>
  );
}

const clock = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
};

/// Ticks once a second while mounted, so the card itself never re-renders on a clock.
function Elapsed({ since }: { since?: string }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const start = since ? Date.parse(since) : NaN;
  return <span className="num">{Number.isFinite(start) ? clock(now - start) : ""}</span>;
}

const pick = <T,>(lane: string, index: number, f: (t: Turn) => T) => (s: ReturnType<typeof useStore.getState>) => {
  const t = s.lanes[lane]?.conv.turns[index];
  return t ? f(t) : undefined;
};

export const TurnView = memo(function TurnView({ lane, index }: { lane: string; index: number }) {
  const t = useStore(pick(lane, index, (t) => t));
  const waiting = useStore((s) => (s.lanes[lane]?.pending.length ?? 0) > 0);
  const number = index + 1;
  if (!t) return null;
  const state = stateOf(t, waiting);
  const look = LOOK[state];
  if (state === "quiet") return <QuietLine lane={lane} index={index} />;
  const cmd = t.gate?.command?.split("&&")[0].trim();
  const calls = Object.values(t.calls).filter((c) => !c.parent);
  const ms = t.ms ?? t.usage?.ms;
  const job = t.prompt?.startsWith(JOB_PREFIX);
  return (
    <article className="turn" data-tone={look.tone}>
      <span className="dot" aria-hidden />
      <div className="turn-body">
        {job ? <pre className="report">{t.prompt!.slice(0, 300)}</pre> : <p className={`turn-prompt${t.prompt ? "" : " none"}`}>{promptLine(t.prompt).slice(0, 400)}</p>}
        <div className="meta">
          <span className="state">
            {look.word}
            {state === "committed" && t.commit && <span className="mono"> {t.commit.slice(0, 7)}</span>}
            {state === "checking" && cmd && <span className="mono"> {cmd.length > 24 ? cmd.slice(0, 24) + "…" : cmd}</span>}
            {state === "failed" && t.gate?.problems?.length ? ` · ${t.gate.problems.length}` : ""}
          </span>
          {!t.closed ? <Elapsed since={t.startedAt} /> : ms ? <span className="num">{clock(ms)}</span> : null}
          {t.usage && <Tokens u={t.usage} />}
          <span className="num">{time(t.startedAt)}</span>
          {t.closed && t.snapshot && <Rewind lane={lane} turn={t} number={number} />}
        </div>
        {state === "failed" && <Problems turn={t} />}
        {t.failures
          .filter((f) => !f.history)
          .map((f, i) => (
            <div key={i} className="note error">
              {f.message}
              {/auth|oauth/.test(f.kind) && (
                <button className="link" onClick={() => useStore.setState({ settings: true })}>
                  Log in →
                </button>
              )}
            </div>
          ))}
        {t.files.length > 0 && <Files lane={lane} id={t.id} files={t.files} commit={t.commit} />}
        {calls.length > 0 && <Commands lane={lane} index={index} all={t.calls} open={!t.closed} />}
      </div>
    </article>
  );
});

/// A count as a person reads it: 840, 12.4k, 1.2M. `0k` for 400 tokens said nothing was sent.
export const count = (n: number) => (n < 1000 ? `${n}` : n < 1e6 ? `${(n / 1000).toFixed(n < 1e4 ? 1 : 0)}k` : `${(n / 1e6).toFixed(1)}M`);

/// What a turn spent, as one figure on the meta line — the context it ended at, which is what
/// decides whether the next turn fits — and every number the stream gave, labelled, on hover.
/// Fresh input, cache reads and cache writes are billed differently; one "in" that hid two of them
/// was a number nobody could check against a bill or a limit. A click copies the lot.
function Tokens({ u }: { u: Usage }) {
  const read = u.input + u.cache_read + u.cache_write;
  const detail = [
    `fresh input   ${u.input.toLocaleString()}`,
    `cache read    ${u.cache_read.toLocaleString()}`,
    `cache write   ${u.cache_write.toLocaleString()}`,
    `output        ${u.output.toLocaleString()}`,
    `context       ${u.context.toLocaleString()}`,
    `from cache    ${read ? Math.round((100 * u.cache_read) / read) : 0}%`,
    u.ms ? `duration      ${(u.ms / 1000).toFixed(1)}s` : "",
    u.cost !== undefined ? `cost          $${u.cost.toFixed(4)}` : "",
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <button className="meta-item num" title={`${detail}\n\nClick to copy`} onClick={() => void navigator.clipboard.writeText(detail)}>
      {u.context ? `${count(u.context)} ctx` : `${count(u.output)} out`}
      {u.cost !== undefined ? ` · $${u.cost.toFixed(2)}` : ""}
    </button>
  );
}

function Problems({ turn }: { turn: Turn }) {
  const problems = turn.gate?.problems ?? [];
  const [copied, setCopied] = useState(false);
  if (!problems.length) return null;
  function copy() {
    const text =
      "The project's checks failed. Fix these:\n\n" +
      problems
        .slice(0, 30)
        .map((p) => `${p.file ?? ""}${p.line ? `:${p.line}` : ""} ${p.message ?? ""}`.trim())
        .join("\n");
    void navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    });
  }
  return (
    <div className="problems">
      <div className="problems-head">
        <span>{problems.length} problem{problems.length === 1 ? "" : "s"}</span>
        <button className="link small" onClick={copy}>
          {copied ? "Copied" : "Copy for the agent"}
        </button>
      </div>
      {problems.slice(0, 20).map((p, i) => (
        <div key={i} className="problem">
          <span>{p.message}</span>
          <span className="where">
            {p.file}
            {p.line ? `:${p.line}` : ""}
          </span>
        </div>
      ))}
      {problems.length > 20 && <div className="small faint">and {problems.length - 20} more</div>}
    </div>
  );
}

/// A path as the person reads it: relative to the lane's checkout or the project, absolute only
/// when it really is outside both.
function relative(path: string, roots: string[]): string {
  for (const root of roots) if (root && path.startsWith(root + "/")) return path.slice(root.length + 1);
  return path;
}

const SHOWN = 6;

/// Memoised on what it draws: the card re-renders for every streamed append to its turn, and this
/// list and every open diff in it have nothing to do with the text.
const Files = memo(function Files({ lane, id, files: all_, commit }: { lane: string; id: string; files: string[]; commit?: string }) {
  const [all, setAll] = useState(false);
  const roots = useStore((s) => {
    const l = s.lanes[lane];
    return l ? [l.wt ? `${l.project}/.keel/worktrees/${l.wt}` : "", l.project].join("\n") : "";
  }).split("\n");
  const files = [...new Set(all_.map((f) => relative(f, roots)))];
  const shown = all ? files.slice(0, 200) : files.slice(0, SHOWN);
  return (
    <div className="files">
      {shown.map((f) => (
        <FileRow key={f} lane={lane} id={id} commit={commit} path={f} />
      ))}
      {files.length > shown.length && (
        <button className="more" onClick={() => setAll(true)}>
          {files.length - shown.length} more file{files.length - shown.length === 1 ? "" : "s"}
        </button>
      )}
    </div>
  );
});

const INLINE_ROWS = 400;

const FileRow = memo(function FileRow({ lane, id, commit, path }: { lane: string; id: string; commit?: string; path: string }) {
  const [open, setOpen] = useState(false);
  // Keyed by the commit it was read for: opened while the checks ran, the row showed the working
  // tree, and once the commit landed it went on presenting that as the commit's diff.
  const [read, setRead] = useState<{ for?: string; diff: Prepared | null }>();
  const diff = read && read.for === commit ? read.diff : undefined;
  const latest = useStore((s) => s.lanes[lane]?.conv.turns.at(-1)?.id === id);
  const outside = path.startsWith("/");
  const slash = path.lastIndexOf("/");
  useEffect(() => {
    if (!open || diff !== undefined || outside) return;
    const l = useStore.getState().lanes[lane];
    const ep = l && useStore.getState().projects[l.project]?.endpoint;
    if (!ep) return;
    // A committed turn is its commit; one not committed is the file as it is now.
    const fetched = commit
      ? get<DiffResponse[]>(ep, "/api/git/commit/diff", { sha: commit, path, wt: l.wt }).then((d) => d[0])
      : get<DiffResponse>(ep, "/api/git/diff", { path, wt: l.wt });
    fetched.then(
      (d) => setRead({ for: commit, diff: d ? prepare(d) : null }),
      () => setRead({ for: commit, diff: null }),
    );
  }, [open, diff, outside, lane, commit, path]);
  return (
    <>
      <button className="file-row" aria-expanded={open} onClick={() => setOpen(!open)} title={path}>
        <Icon name="file" size={13} className="file-icon" />
        <span className="file-name">
          {path.slice(slash + 1)}
          {slash > 0 && <span className="dir">{path.slice(0, slash)}</span>}
        </span>
        {diff && (
          <span className="num stat">
            <span className="add">+{diff.adds}</span> <span className="del">−{diff.dels}</span>
          </span>
        )}
      </button>
      {open &&
        (outside ? (
          <div className="small faint" style={{ padding: "2px 24px" }}>
            Outside this repository — <code>{path}</code>
          </div>
        ) : diff === undefined ? (
          <div className="small faint" style={{ padding: "2px 24px" }}>Reading the diff…</div>
        ) : diff === null ? (
          <div className="small faint" style={{ padding: "2px 24px" }}>No diff to show — the file may have been reverted since.</div>
        ) : (
          <>
            {!commit && (
              <div className="small faint" style={{ padding: "2px 24px" }}>
                uncommitted — the file as it is now{latest ? "" : "; may include later turns' edits"}
              </div>
            )}
            <InlineDiff rows={diff.rows} limit={INLINE_ROWS} />
            {diff.rows.length > INLINE_ROWS && (
              <div className="small faint" style={{ padding: "0 24px 6px" }}>
                Showing {INLINE_ROWS} of {diff.rows.length.toLocaleString()} lines —{" "}
                <button className="link small" onClick={() => void useStore.getState().openDiff(lane, path)}>
                  Open in Changes
                </button>
              </div>
            )}
          </>
        ))}
    </>
  );
});

function glyph(c: Call): IconName {
  if (c.state === "failed") return "octagon";
  if (/^(Read|Grep|Glob|LS|WebFetch|WebSearch)$/.test(c.tool)) return "eye";
  if (/^(Edit|Write|MultiEdit|NotebookEdit|Update)$/.test(c.tool)) return "pencil";
  if (/^(Task|Agent)$/.test(c.tool)) return "users";
  return "terminal";
}

/// Memoised on the turn's call map, which the reducer replaces only when a call changes — not for
/// the text appends that re-render the card around it.
const Commands = memo(function Commands({ lane, index, all, open: initial }: { lane: string; index: number; all: Record<string, Call>; open: boolean }) {
  const calls = useMemo(() => Object.values(all).filter((c) => !c.parent), [all]);
  const [open, setOpen] = useState(initial);
  const failed = calls.filter((c) => c.state === "failed").length;
  return (
    <div className="calls">
      <button className="more" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Icon name="chevron-right" size={11} className="chev" />
        {calls.length} tool call{calls.length === 1 ? "" : "s"}
        {failed > 0 && <span className="del"> · {failed} failed</span>}
      </button>
      {open && calls.slice(-200).map((c) => <CommandRow key={c.id} lane={lane} index={index} call={c} />)}
    </div>
  );
});

const CommandRow = memo(function CommandRow({ lane, index, call, depth = 0 }: { lane: string; index: number; call: Call; depth?: number }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <CommandLine call={call} depth={depth} open={open} toggle={() => setOpen(!open)} />
      {open && <CommandDetail lane={lane} index={index} call={call} depth={depth} />}
    </>
  );
});

function CommandLine({ call, depth, open, toggle }: { call: Call; depth: number; open: boolean; toggle: () => void }) {
  return (
    <button className="cmd-row" style={{ paddingLeft: 4 + depth * 16 }} aria-expanded={open} onClick={toggle}>
      <span className="glyph" style={call.state === "failed" ? { color: "var(--del)" } : undefined}>
        {call.state === "running" ? <span className="busy-dot" /> : <Icon name={glyph(call)} size={12} />}
      </span>
      <span className="cmd-tool">{call.tool}</span>
      <span className="cmd-subject">{call.subject || call.reason || ""}</span>
      {call.state === "interrupted" && <span className="small faint">interrupted</span>}
    </button>
  );
}

/// Mounted only when a row is open: serialising a `Write`'s whole file on every streamed batch
/// was work per frame for a row nobody had opened.
function CommandDetail({ lane, index, call, depth }: { lane: string; index: number; call: Call; depth: number }) {
  const children = useStore(pick(lane, index, (t) => t.children[call.id]));
  const calls = useStore(pick(lane, index, (t) => t.calls));
  const input = useMemo(() => (call.input ? JSON.stringify(call.input, null, 2) : ""), [call.input]);
  return (
    <>
      <div style={{ paddingLeft: 28 + depth * 16 }}>
        {input && <pre className="code">{input.length > 20_000 ? input.slice(0, 20_000) + "\n…" : input}</pre>}
        {call.output !== undefined && (
          <pre className="code">
            {call.output.split("\n").slice(0, 300).join("\n") || "(no output)"}
            {call.cut ? `\n… ${call.cut.toLocaleString()} bytes not shown` : ""}
          </pre>
        )}
      </div>
      {children?.map((id) => calls?.[id] && <CommandRow key={id} lane={lane} index={index} call={calls[id]} depth={depth + 1} />)}
    </>
  );
}

/// Put the checkout back as it was before this turn ran. Undoable, and refused while the agent
/// is mid-turn, which would be two writers on one tree.
function Rewind({ lane, turn, number }: { lane: string; turn: Turn; number: number }) {
  const [asking, setAsking] = useState(false);
  // Anyone writing this tree, not just this lane: the daemon refuses too, this says why first.
  const busy = useStore((s) => {
    const me = s.lanes[lane];
    return !!me && Object.values(s.lanes).some((o) => o.project === me.project && o.wt === me.wt && o.running);
  });
  async function rewind() {
    const l = useStore.getState().lanes[lane];
    const ep = l && useStore.getState().projects[l.project]?.endpoint;
    if (!ep || !turn.snapshot) return;
    setAsking(false);
    try {
      const r = await post<{ undo: string }>(ep, "/api/git/restore", { tree: turn.snapshot }, { wt: l.wt });
      useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], rewound: { number, undo: r.undo } } } }));
    } catch (e) {
      useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], error: `Could not rewind: ${e instanceof Error ? e.message : e}` } } }));
    }
  }
  return (
    <span style={{ position: "relative" }}>
      <button className="ghost rewind" disabled={busy} onClick={() => setAsking(!asking)} aria-label={`Rewind files to before turn ${number}`} title={busy ? "An agent is writing this tree — rewind when it is idle" : `Rewind files to before turn ${number}`}>
        <Icon name="rewind" size={13} />
      </button>
      {asking && (
        <>
          <div className="scrim" onClick={() => setAsking(false)} />
          <div className="popover" style={{ position: "absolute", right: 0, top: 28 }}>
            <p style={{ margin: "0 0 8px" }}>
              <strong>Rewind files to before turn {number}?</strong> Restores the whole checkout — {turn.files.length} file{turn.files.length === 1 ? "" : "s"} this turn changed, plus anything since. Undoable. claude's conversation still remembers the edits; tell it, or use <code>/rewind</code> in the terminal.
            </p>
            <div className="actions">
              <button className="primary" onClick={rewind}>
                Rewind files
              </button>
              <button onClick={() => setAsking(false)}>Cancel</button>
            </div>
          </div>
        </>
      )}
    </span>
  );
}
