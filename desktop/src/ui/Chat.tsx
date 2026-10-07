import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Virtuoso, type VirtuosoHandle, type StateSnapshot } from "react-virtuoso";
import { useStore } from "../store";
import { Markdown, Copy } from "./Markdown";
import { Icon } from "./icons";
import { BrandMark } from "./BrandMark";

type Row = { turn: number; step: string; key: string };
const scrolls = new Map<string, StateSnapshot>();
const expanded = new Set<string>();
const following = new Map<string, boolean>();

export function Chat({ lane }: { lane: string }) {
  // Only the shape of the conversation subscribes here. Tokens update their own block.
  const shape = useStore((s) => JSON.stringify((s.lanes[lane]?.conv.turns ?? []).map((t) => [t.id, t.steps])));
  const rows = useMemo(() => (JSON.parse(shape) as [string, string[]][]).flatMap(([id, steps], turn) => [
    { turn, step: "prompt", key: `${id}:prompt` }, ...steps.map((step) => ({ turn, step, key: `${id}:${step}` })), { turn, step: "end", key: `${id}:end` },
  ]), [shape]);
  const loaded = useStore((s) => s.lanes[lane]?.loaded);
  const running = useStore((s) => s.lanes[lane]?.running);
  const agent = useStore((s) => s.lanes[lane]?.agent);
  const list = useRef<VirtuosoHandle>(null);
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
  if (!rows.length) return <div className="chat-empty"><div className="chat-empty-mark"><Icon name="chat" size={24} /></div>
    <h2>{!loaded ? "Opening your conversation…" : running ? "Starting your agent…" : "What are we building?"}</h2>
    <p>{loaded && !running ? `Work with ${agent === "codex" ? "Codex" : "Claude"} in this lane. Ask a question, attach context, or start a task.` : "Your conversation and workspace stay together."}</p>
    {loaded && !running && <span className="small faint">@ for files · / for commands · Shift+Enter for a new line</span>}</div>;
  return <div className="chat-transcript" aria-label="Conversation">
    <Virtuoso ref={list} scrollerRef={scrollerRef} data={rows} computeItemKey={(_, row) => row.key}
      {...(saved.current ? { restoreStateFrom: saved.current } : { initialTopMostItemIndex: rows.length - 1 })}
      rangeChanged={remember} isScrolling={(scrolling) => { if (!scrolling) remember(); }}
      followOutput={() => follow.current ? "auto" : false} totalListHeightChanged={stick} atBottomStateChange={setBottom} atBottomThreshold={4}
      increaseViewportBy={300} itemContent={(_, row) => <ChatRow lane={lane} row={row} />} />
    {!bottom && <button className="chat-jump" onClick={() => { follow.current = true; following.set(lane, true); stick(); }}>Jump to latest ↓</button>}
  </div>;
}

const ChatRow = memo(function ChatRow({ lane, row }: { lane: string; row: Row }) {
  if (row.step === "prompt") return <Prompt lane={lane} index={row.turn} />;
  if (row.step === "end") return <End lane={lane} index={row.turn} />;
  if (row.step.startsWith("b:")) return <Block lane={lane} index={row.turn} step={row.step.slice(2)} />;
  return <Activity lane={lane} index={row.turn} id={row.step.slice(2)} />;
});

function Prompt({ lane, index }: { lane: string; index: number }) {
  const text = useStore((s) => s.lanes[lane]?.conv.turns[index]?.prompt);
  return <article className="chat-row chat-user"><div className="chat-speaker">You <Copy text={text ?? ""} /></div><p>{text ?? "Earlier in the conversation"}</p></article>;
}

function Block({ lane, index, step }: { lane: string; index: number; step: string }) {
  const block = useStore((s) => s.lanes[lane]?.conv.turns[index]?.blocks[step]);
  const first = useStore((s) => s.lanes[lane]?.conv.turns[index]?.steps.find((key) => key.startsWith("b:") && s.lanes[lane]?.conv.turns[index]?.blocks[key.slice(2)]?.kind === "say") === `b:${step}`);
  const agent = useStore((s) => s.lanes[lane]?.agent);
  const key = `${lane}:${index}:${step}`;
  if (!block) return null;
  return <div className="chat-row">{first && <div className="assistant-speaker"><span><BrandMark size={14} /></span>{agent === "codex" ? "Codex" : "Claude"}</div>}{block.kind === "think" ? <details className="chat-thinking" open={expanded.has(key)} onToggle={(e) => e.currentTarget.open ? expanded.add(key) : expanded.delete(key)}><summary>Reasoning</summary><Markdown text={block.text} /></details> : <Markdown text={block.text} />}</div>;
}

function Activity({ lane, index, id }: { lane: string; index: number; id: string }) {
  const call = useStore((s) => s.lanes[lane]?.conv.turns[index]?.calls[id]);
  const children = useStore((s) => s.lanes[lane]?.conv.turns[index]?.children[id]);
  const key = `${lane}:${index}:${id}`;
  const [open, setOpen] = useState(expanded.has(key));
  if (!call) return null;
  return <div className="chat-row chat-activity"><details open={open} onToggle={(e) => { setOpen(e.currentTarget.open); if (e.currentTarget.open) expanded.add(key); else expanded.delete(key); }}>
    <summary><span className={`activity-dot ${call.state}`} /><span>{call.tool}</span><span className="activity-subject">{call.subject}</span><span className="small faint">{call.state}</span></summary>
    {open && <div className="activity-body">{call.reason && <p>{call.reason}</p>}{call.input && <><Copy text={JSON.stringify(call.input, null, 2)} /><pre tabIndex={0}>{JSON.stringify(call.input, null, 2)}</pre></>}
      {call.output && <><Copy text={call.output} /><pre tabIndex={0}>{call.output}</pre></>}{!!call.cut && <p className="small faint">Output truncated by the agent stream ({call.cut} characters omitted).</p>}
      {children?.map((child) => <Activity key={child} lane={lane} index={index} id={child} />)}</div>}
  </details></div>;
}

function End({ lane, index }: { lane: string; index: number }) {
  const closed = useStore((s) => s.lanes[lane]?.conv.turns[index]?.closed);
  const failures = useStore((s) => s.lanes[lane]?.conv.turns[index]?.failures);
  const files = useStore((s) => s.lanes[lane]?.conv.turns[index]?.files);
  const usage = useStore((s) => s.lanes[lane]?.conv.turns[index]?.usage);
  return <div className="chat-row chat-end">{failures?.filter((f) => !f.history).map((f, i) => <p role="alert" className="error" key={i}>{f.message}</p>)}
    {closed && <div className="chat-turn-meta"><span>{closed === "done" ? "Completed" : closed}</span>{usage && <span>{usage.context.toLocaleString()} context tokens</span>}{!!files?.length && <button className="link" onClick={() => useStore.setState({ panelTab: "git" })}>{files.length} changed {files.length === 1 ? "file" : "files"} →</button>}</div>}</div>;
}
