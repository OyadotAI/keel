// The virtual conversation follows streaming output without taking control of manual scrolling.
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { shownTool } from "../browser";
import { Virtuoso, type VirtuosoHandle, type StateSnapshot } from "react-virtuoso";
import { AGENT_NAME, useStore } from "../store";
import { Markdown, Copy } from "./Markdown";
import { Icon } from "./icons";
import { BrandMark } from "./BrandMark";
import { DocumentImage } from "./MarkdownPreview";
import { promptAttachments } from "../prompt-attachments";
import { Working } from "./Working";
import type { Turn } from "../reduce";
import { duration, turnFacts } from "../turn-facts";

/** How a turn ended, in words: the decoder's reasons are for code. */
const ENDINGS: Record<string, string> = { done: "Done", stopped: "Stopped", interrupted: "Stopped", failed: "Didn't finish", superseded: "Done" };
import { InlineDiff } from "./InlineDiff";
import { describe, toolView, DIFF_ROWS, STATE_WORDS, type ToolView } from "../tool-view";

/** Lines of an edit shown before it is opened. */
const GLANCE_ROWS = 12;

type Row = { turn: number; step: string; key: string };
const scrolls = new Map<string, StateSnapshot>();
const expanded = new Set<string>();
const following = new Map<string, boolean>();
const TURN = "\u0002";
const STEP = "\u0001";
const shapes = new WeakMap<Turn, string>();
/** A turn's id and the steps that draw a row; empty reasoning draws nothing. */
function shapeOf(t: Turn): string {
  let shape = shapes.get(t);
  if (shape === undefined) {
    const steps = t.steps.filter((step) => {
      const block = step.startsWith("b:") ? t.blocks[step.slice(2)] : undefined;
      return block?.kind !== "think" || !!block.text.trim();
    });
    shape = [t.id, ...steps].join(STEP);
    shapes.set(t, shape);
  }
  return shape;
}

export function Chat({ lane }: { lane: string }) {
  // Only the shape of the conversation subscribes here. Tokens update their own block.
  // The turns array changes only when this lane's conversation does; each turn's shape is cached
  // by identity, so a token re-reads one turn rather than serialising all of them — which this
  // used to do on every store update from any lane.
  const turns = useStore((s) => s.lanes[lane]?.conv.turns);
  const shape = useMemo(() => (turns ?? []).map(shapeOf).join(TURN), [turns]);
  const rows = useMemo(() => (shape ? shape.split(TURN) : []).flatMap((one, turn) => {
    const [id, ...steps] = one.split(STEP);
    return [{ turn, step: "prompt", key: `${id}:prompt` }, ...steps.map((step) => ({ turn, step, key: `${id}:${step}` })), { turn, step: "end", key: `${id}:end` }];
  }), [shape]);
  const loaded = useStore((s) => s.lanes[lane]?.loaded);
  const running = useStore((s) => s.lanes[lane]?.running);
  const agent = useStore((s) => s.lanes[lane]?.agent);
  const list = useRef<VirtuosoHandle>(null);
  // Rows that existed when the lane opened never animate: Virtuoso remounts a row each time it
  // scrolls back into view, and an old turn rising again on every scroll is noise, not life.
  const born = useRef(rows.length);
  const [bottom, setBottom] = useState(true);
  const saved = useRef(scrolls.get(lane));
  const follow = useRef(following.get(lane) ?? true);
  const frame = useRef(0);
  const [scroller, setScroller] = useState<HTMLElement | null>(null);
  const scrollerRef = useCallback((node: HTMLElement | Window | null) => setScroller(node instanceof HTMLElement ? node : null), []);
  const stick = useCallback(() => {
    cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => {
      if (follow.current) list.current?.scrollToIndex({ index: "LAST", align: "end", behavior: "auto" });
    });
  }, []);
  useEffect(() => {
    if (!scroller) return;
    // Content growth is not a user scroll. Only user input can detach the tail;
    // reaching the bottom (or Jump to latest) attaches it again.
    const pause = () => { follow.current = false; following.set(lane, false); };
    const wheel = (e: WheelEvent) => { if (e.deltaY < 0) pause(); };
    const key = (e: KeyboardEvent) => { if (["ArrowUp", "PageUp", "Home"].includes(e.key)) pause(); };
    const pointer = (e: PointerEvent) => { if (e.target === scroller) pause(); };
    const scroll = () => {
      if (scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <= 4) {
        follow.current = true; following.set(lane, true);
      }
    };
    scroller.addEventListener("wheel", wheel, { passive: true });
    scroller.addEventListener("touchmove", pause, { passive: true });
    scroller.addEventListener("keydown", key);
    scroller.addEventListener("pointerdown", pointer);
    scroller.addEventListener("scroll", scroll, { passive: true });
    const resize = new ResizeObserver(stick);
    resize.observe(scroller);
    stick();
    return () => {
      resize.disconnect(); cancelAnimationFrame(frame.current);
      scroller.removeEventListener("wheel", wheel);
      scroller.removeEventListener("touchmove", pause);
      scroller.removeEventListener("keydown", key);
      scroller.removeEventListener("pointerdown", pointer);
      scroller.removeEventListener("scroll", scroll);
    };
  }, [lane, scroller, stick]);
  const remember = () => list.current?.getState((state) => scrolls.set(lane, state));
  if (!rows.length) return <div className="chat-empty" data-reference-target={lane}><div className="chat-empty-mark">{running ? <Working /> : <Icon name="chat" size={24} />}</div>
    <h2>{!loaded ? "Opening your conversation…" : running ? "Starting your agent…" : "What are we building?"}</h2>
    <p>{loaded && !running ? `Work with ${AGENT_NAME[agent ?? "claude"]} in this lane. Ask a question, attach context, or start a task.` : "Your conversation and workspace stay together."}</p>
    {loaded && !running && <span className="small faint">@ for files · / for commands · Shift+Enter for a new line</span>}</div>;
  return <div className="chat-transcript" aria-label="Conversation" data-reference-target={lane}>
    <Virtuoso ref={list} scrollerRef={scrollerRef} data={rows} computeItemKey={(_, row) => row.key}
      {...(saved.current ? { restoreStateFrom: saved.current } : { initialTopMostItemIndex: rows.length - 1 })}
      rangeChanged={remember} isScrolling={(scrolling) => { if (!scrolling) remember(); }}
      followOutput={() => follow.current ? "auto" : false} totalListHeightChanged={stick} atBottomStateChange={setBottom} atBottomThreshold={4}
      increaseViewportBy={300} itemContent={(i, row) => <ChatRow lane={lane} row={row} fresh={i >= born.current} streaming={!!running && i === rows.length - 2} />} />
    {!bottom && <button className="chat-jump" onClick={() => { follow.current = true; following.set(lane, true); stick(); }}>Jump to latest ↓</button>}
  </div>;
}

const ChatRow = memo(function ChatRow({ lane, row, fresh, streaming }: { lane: string; row: Row; fresh: boolean; streaming: boolean }) {
  return <div className={`chat-item${fresh ? " is-fresh" : ""}${streaming ? " is-streaming" : ""}`}>{rowContent(lane, row)}</div>;
});

function rowContent(lane: string, row: Row) {
  if (row.step === "prompt") return <Prompt lane={lane} index={row.turn} />;
  if (row.step === "end") return <End lane={lane} index={row.turn} />;
  if (row.step.startsWith("b:")) return <Block lane={lane} index={row.turn} step={row.step.slice(2)} />;
  return <Activity lane={lane} index={row.turn} id={row.step.slice(2)} />;
}

function Prompt({ lane, index }: { lane: string; index: number }) {
  const prompt = useStore((s) => s.lanes[lane]?.conv.turns[index]?.prompt);
  const ep = useStore((s) => s.projects[s.lanes[lane]?.project ?? ""]?.endpoint);
  const wt = useStore((s) => s.lanes[lane]?.wt);
  const { text, images } = useMemo(() => promptAttachments(prompt ?? ""), [prompt]);
  return <article className="chat-row chat-user"><div className="chat-speaker">You <Copy text={text} /></div>
    {text ? <p>{text}</p> : !images.length && <p>Earlier in the conversation</p>}
    {ep && images.length > 0 && <div className="chat-images">{images.map((path) => <DocumentImage key={path} ep={ep} wt={wt} path={path} alt={path.split("/").pop() ?? ""} revision={0} />)}</div>}
  </article>;
}

function Block({ lane, index, step }: { lane: string; index: number; step: string }) {
  const block = useStore((s) => s.lanes[lane]?.conv.turns[index]?.blocks[step]);
  const first = useStore((s) => s.lanes[lane]?.conv.turns[index]?.steps.find((key) => key.startsWith("b:") && s.lanes[lane]?.conv.turns[index]?.blocks[key.slice(2)]?.kind === "say") === `b:${step}`);
  const agent = useStore((s) => s.lanes[lane]?.agent);
  const key = `${lane}:${index}:${step}`;
  const [open, setOpen] = useState(expanded.has(key));
  // Older saved conversations may contain reasoning items without displayable text.
  if (!block || (block.kind === "think" && !block.text.trim())) return null;
  if (block.kind === "notice") return <div className="chat-row chat-notice" role="note"><span>{block.text}</span></div>;
  if (block.kind === "you") return <article className="chat-row chat-user chat-steer"><div className="chat-speaker">You · while it worked <Copy text={block.text} /></div><p>{block.text}</p></article>;
  return <div className="chat-row">{first && <div className="assistant-speaker"><span><BrandMark size={14} /></span>{AGENT_NAME[agent ?? "claude"]}</div>}{block.kind === "think" ? <details className="chat-thinking" open={open} onToggle={(e) => { setOpen(e.currentTarget.open); if (e.currentTarget.open) expanded.add(key); else expanded.delete(key); }}><summary>Reasoning</summary><Markdown text={block.text} /></details> : <Markdown text={block.text} />}</div>;
}

function Activity({ lane, index, id }: { lane: string; index: number; id: string }) {
  const call = useStore((s) => s.lanes[lane]?.conv.turns[index]?.calls[id]);
  const children = useStore((s) => s.lanes[lane]?.conv.turns[index]?.children[id]);
  const key = `${lane}:${index}:${id}`;
  const [open, setOpen] = useState(expanded.has(key));
  const view = useMemo(() => toolView(call?.tool ?? "", call?.input), [call?.tool, call?.input]);
  const said = useMemo(() => describe(call?.tool ?? "", call?.subject ?? "", call?.input), [call?.tool, call?.subject, call?.input]);
  if (!call) return null;
  return <div className="chat-row chat-activity"><details open={open} onToggle={(e) => { setOpen(e.currentTarget.open); if (e.currentTarget.open) expanded.add(key); else expanded.delete(key); }}>
    <summary><span className={`activity-dot ${call.state}`} /><span className="activity-verb">{said.verb === call.tool ? shownTool(call.tool) : said.verb}</span><span className="activity-subject">{said.object}</span>
      {view.kind === "diff" && <span className="activity-counts"><span className="adds">+{view.adds}</span> <span className="dels">−{view.dels}</span></span>}
      <ChildProgress lane={lane} index={index} ids={children} />{STATE_WORDS[call.state] !== "" && <span className={`small activity-state ${call.state}`}>{STATE_WORDS[call.state] ?? call.state}</span>}</summary>
    {open && <div className="activity-body">{call.reason && <p>{call.reason}</p>}
      {view.kind === "raw" && call.input && <><Copy text={JSON.stringify(call.input, null, 2)} /><pre tabIndex={0}>{JSON.stringify(call.input, null, 2)}</pre></>}
      {view.kind === "command" && <pre className="activity-command" tabIndex={0}>$ {view.command}</pre>}
      {view.kind === "diff" && <InlineDiff rows={view.rows} path={view.path} limit={DIFF_ROWS} />}
      {call.output && <><Copy text={call.output} /><pre tabIndex={0}>{call.output}</pre></>}{!!call.cut && <p className="small faint">Output truncated by the agent stream ({call.cut} characters omitted).</p>}
      {children?.map((child) => <Activity key={child} lane={lane} index={index} id={child} />)}</div>}
  </details>{!open && <Glance view={view} />}</div>;
}

/** What the CLI shows without a click: an edit's first lines, the whole todo list. */
function Glance({ view }: { view: ToolView }) {
  if (view.kind === "todos") return <ul className="chat-todos">{view.todos.map((t, i) => <li key={i} className={t.status}><span aria-hidden="true">{t.status === "completed" ? "✓" : t.status === "in_progress" ? "◐" : "○"}</span>{t.text}</li>)}</ul>;
  if (view.kind === "diff" && view.rows.length) return <div className="chat-glance"><InlineDiff rows={view.rows} path={view.path} limit={GLANCE_ROWS} />{view.rows.length > GLANCE_ROWS && <span className="small faint">{view.rows.length - GLANCE_ROWS} more lines — open to see all</span>}</div>;
  return null;
}

/** A subagent's progress on its own row: how many steps, and the one it is on. */
function ChildProgress({ lane, index, ids }: { lane: string; index: number; ids?: string[] }) {
  const last = useStore((s) => (ids?.length ? s.lanes[lane]?.conv.turns[index]?.calls[ids[ids.length - 1]] : undefined));
  if (!ids?.length) return null;
  return <span className="activity-children small faint">{ids.length} {ids.length === 1 ? "step" : "steps"}{last && last.state === "running" ? ` · ${describe(last.tool, last.subject, last.input).verb} ${describe(last.tool, last.subject, last.input).object}` : ""}</span>;
}

function End({ lane, index }: { lane: string; index: number }) {
  const closed = useStore((s) => s.lanes[lane]?.conv.turns[index]?.closed);
  const failures = useStore((s) => s.lanes[lane]?.conv.turns[index]?.failures);
  const files = useStore((s) => s.lanes[lane]?.conv.turns[index]?.files);
  const usage = useStore((s) => s.lanes[lane]?.conv.turns[index]?.usage);
  const ms = useStore((s) => s.lanes[lane]?.conv.turns[index]?.ms);
  return <div className="chat-row chat-end">{failures?.filter((f) => !f.history).map((f, i) => <p role="alert" className="error" key={i}>{f.message}</p>)}
    {closed && <div className="chat-turn-meta" title={turnFacts(usage, ms ?? usage?.ms).join(" · ")}><span>{ENDINGS[closed] ?? closed}{duration(ms ?? usage?.ms)}</span>{!!files?.length && <button className="link" onClick={() => useStore.setState({ panelTab: "git" })}>{files.length} {files.length === 1 ? "file" : "files"} changed — review →</button>}</div>}</div>;
}

