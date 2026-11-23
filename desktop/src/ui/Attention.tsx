// One current-state list opens existing recovery surfaces without answering or dismissing them.
import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "../store";
import { attentionCoverage, attentionItems, type AttentionItem } from "../attention";
import { Dialog } from "./kit";
import { Icon, type IconName } from "./icons";

const REASON_ICON: Record<string, IconName> = { input: "hand", error: "x-circle", checks: "x-circle", update: "list" };

/** A primitive selector keeps streaming text in other lanes from rerendering the entry. */
export function AttentionButton({ open }: { open: () => void }) {
  const count = useStore((state) => attentionItems(state).length);
  return <button className="attention-entry" onClick={open} aria-haspopup="dialog" aria-label={`Attention · ${count} ${count === 1 ? "lane" : "lanes"}`}>
    <Icon name="list" size={16} /><span>Attention</span><span className="attention-count">{count}</span>
  </button>;
}

/** This plain-text row keeps every reason readable while offering just its current destination. */
export function AttentionRow({ item, first, activate }: { item: AttentionItem; first: boolean; activate: (lane: string) => void }) {
  return <li className="attention-row" data-attention-row={item.lane}>
    <div className="attention-identity">
      <span className="attention-project" title={item.project}>{item.projectName}</span>
      <strong className="attention-title" title={item.title}>{item.title}</strong>
      <ul className="attention-reasons">{item.reasons.map((reason, index) => <li key={`${reason.kind}:${index}`} className={`attention-reason ${reason.kind}`}>
        <Icon name={REASON_ICON[reason.kind]} size={14} /><span>{reason.text}</span>
      </li>)}</ul>
      {item.disconnected && <p className="attention-disconnected">Disconnected — details may be out of date.</p>}
    </div>
    <div className="actions"><button className={first ? "primary" : undefined} onClick={() => activate(item.lane)} aria-label={`${item.action} — ${item.projectName} — ${item.title} — ${item.project}`}>{item.action}</button></div>
  </li>;
}

function useAttentionModal(root: React.RefObject<HTMLDivElement | null>, ids: string[]) {
  const focused = useRef<string | undefined>(undefined);
  const previous = useRef(ids);
  useEffect(() => {
    const backdrop = root.current?.closest(".dialog-backdrop");
    const siblings = [...(backdrop?.parentElement?.children ?? [])].filter((element): element is HTMLElement => element instanceof HTMLElement && element !== backdrop);
    const before = siblings.map((element) => element.inert);
    siblings.forEach((element) => { element.inert = true; });
    return () => siblings.forEach((element, index) => { element.inert = before[index]; });
  }, [root]);
  useEffect(() => {
    if (focused.current && !ids.includes(focused.current) && document.activeElement === document.body) {
      const at = previous.current.indexOf(focused.current);
      const next = [...previous.current.slice(at + 1), ...previous.current.slice(0, at).reverse()].find((id) => ids.includes(id));
      const row = next && root.current?.querySelector<HTMLElement>(`[data-attention-row="${CSS.escape(next)}"] button`);
      (row || root.current?.querySelector<HTMLElement>(".attention-close"))?.focus();
    }
    previous.current = ids;
  }, [ids, root]);
  return (target: EventTarget) => { focused.current = (target as HTMLElement).closest<HTMLElement>("[data-attention-row]")?.dataset.attentionRow; };
}

/** Coverage is explicit: opening the dialog does not load dormant lanes or claim global health. */
export function AttentionDialog({ close, activate, openProject }: { close: () => void; activate: (lane: string) => void; openProject: () => Promise<unknown> }) {
  const [picking, setPicking] = useState(false), [error, setError] = useState<string>();
  const order = useStore((state) => state.order), projects = useStore((state) => state.projects), lanes = useStore((state) => state.lanes);
  const source = useMemo(() => ({ order, projects, lanes }), [order, projects, lanes]);
  const items = useMemo(() => attentionItems(source), [source]), coverage = attentionCoverage(source);
  const root = useRef<HTMLDivElement>(null), onFocus = useAttentionModal(root, items.map((item) => item.lane));
  const empty = !coverage.projects ? "Open a project to see questions, errors, failed checks, and new endings here." : coverage.unloaded ? "No loaded updates yet." : "No attention items in loaded conversations.";
  async function pick() {
    setPicking(true); setError(undefined);
    try { await openProject(); } catch { setError("Couldn’t open a project. Try again."); }
    finally { setPicking(false); }
  }
  return <Dialog title="Attention" onClose={close}><div className="attention-content" ref={root} onFocusCapture={(event) => onFocus(event.target)}>
    <p className="attention-subtitle">Across open projects · Updates Keel has received</p>
    <span className="sr-only" role="status" aria-live="polite">{items.length} lanes in Attention</span>
    <div className="attention-scroll">{items.length ? <ol className="attention-list">{items.map((item, index) => <AttentionRow key={item.lane} item={item} first={index === 0} activate={activate} />)}</ol> : <p className="attention-empty">{empty}</p>}</div>
    {!!coverage.unloaded && <p className="attention-coverage">Some conversations haven’t loaded yet. This list includes updates Keel has received.</p>}
    {error && <p role="alert">{error}</p>}
    <div className="actions attention-footer">{!coverage.projects && <button className="primary" disabled={picking} onClick={() => void pick()}>{picking ? "Opening project…" : "Open project…"}</button>}<button className={`attention-close${!items.length && coverage.projects ? " primary" : ""}`} onClick={close}>Close</button></div>
  </div></Dialog>;
}
