// One current-state list opens existing recovery surfaces without answering or dismissing them.
import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "../store";
import { attentionCoverage, attentionItems, type AttentionItem } from "../attention";
import { Dialog } from "./kit";
import { Icon, type IconName } from "./icons";
import { projectDelivery,savedAttention,useDeliveryRevision,openTask,orderAttention } from "../delivery";

function useSavedItems() {
  useDeliveryRevision();
  const projects = useStore((s) => s.projects), order = useStore((s) => s.order);
  return order.flatMap((path) => {
    const project = projects[path], ep = project?.endpoint;
    if (!ep) return [];
    const data = projectDelivery(ep);
    return savedAttention(data?.tasks, data?.schedules).map((item) => ({ ...item, project: path, projectName: project.name, ep, disconnected: !!data?.error }));
  });
}

const REASON_ICON: Record<string, IconName> = { input: "hand", error: "x-circle", checks: "x-circle", update: "list" };

/** A primitive selector keeps streaming text in other lanes from rerendering the entry. */
export function AttentionButton({ open }: { open: () => void }) {
  const liveSignature = useStore((state) => JSON.stringify(attentionItems(state).map((item) => item.lane))), saved = useSavedItems();
  const live: string[] = JSON.parse(liveSignature);
  const count = live.length + saved.filter((item) => !live.some((lane) => item.lanes.includes(lane))).length;
  return <button className="attention-entry" onClick={open} aria-haspopup="dialog" aria-label={`Attention · ${count} ${saved.length ? "items" : count === 1 ? "lane" : "lanes"}`}>
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
export function AttentionDialog({ close, activate, openProject, openRecurring }: { close: () => void; activate: (lane: string) => void; openProject: () => Promise<unknown>; openRecurring?: (project: string) => void }) {
  const [picking, setPicking] = useState(false), [error, setError] = useState<string>();
  const order = useStore((state) => state.order), projects = useStore((state) => state.projects), lanes = useStore((state) => state.lanes);
  const source = useMemo(() => ({ order, projects, lanes }), [order, projects, lanes]);
  const items = useMemo(() => attentionItems(source), [source]), coverage = attentionCoverage(source);
  const saved = useSavedItems(), rows=orderAttention(items,saved,order);
  async function openSaved(item: typeof saved[number]) { try { if (item.recurring) { openRecurring?.(item.project); close(); } else if (item.task) { await openTask(item.project, item.ep, item.task, item.title); close(); } } catch (error) { setError(String(error)); } }
  const root = useRef<HTMLDivElement>(null), onFocus = useAttentionModal(root, rows.map(row=>row.kind==="live"?row.item.lane:`${row.item.project}:${row.item.id}`));
  const empty = !coverage.projects ? "Open a project to see questions, errors, failed checks, and new endings here." : coverage.unloaded ? "No loaded updates yet." : "No attention items in loaded conversations.";
  async function pick() {
    setPicking(true); setError(undefined);
    try { await openProject(); } catch { setError("Couldn’t open a project. Try again."); }
    finally { setPicking(false); }
  }
  return <Dialog title="Attention" onClose={close}><div className="attention-content" ref={root} onFocusCapture={(event) => onFocus(event.target)}>
    <p className="attention-subtitle">Across open projects · Updates Oya Factory has received</p>
    <span className="sr-only" role="status" aria-live="polite">{rows.length} items in Attention</span>
    <div className="attention-scroll">{rows.length ? <ol className="attention-list">{rows.map((row,index)=>row.kind==="live" ? <AttentionRow key={row.item.lane} item={row.item} first={index===0} activate={activate} /> : <SavedAttentionRow key={`${row.item.project}:${row.item.id}`} item={row.item} first={index===0} open={openSaved} />)}</ol> : <p className="attention-empty">{empty}</p>}</div>
    {!!coverage.unloaded && <p className="attention-coverage">Some conversations haven’t loaded yet. This list includes updates Oya Factory has received.</p>}
    {error && <p role="alert">{error}</p>}
    <div className="actions attention-footer">{!coverage.projects && <button className="primary" disabled={picking} onClick={() => void pick()}>{picking ? "Opening project…" : "Open project…"}</button>}<button className={`attention-close${!rows.length && coverage.projects ? " primary" : ""}`} onClick={close}>Close</button></div>
  </div></Dialog>;
}

function SavedAttentionRow({item,first,open}:{item:ReturnType<typeof useSavedItems>[number];first:boolean;open:(item:ReturnType<typeof useSavedItems>[number])=>Promise<void>}) {
  return <li className="attention-row" data-attention-row={`${item.project}:${item.id}`}><div className="attention-identity"><span className="attention-project">{item.projectName}</span><strong className="attention-title">{item.title}</strong><ul className="attention-reasons">{item.reasons.map((reason,index)=><li key={index}>{reason}</li>)}</ul>{item.disconnected&&<p>Disconnected — saved details may be out of date.</p>}</div><button className={first?"primary":undefined} onClick={()=>void open(item)}>{item.recurring?"Review recurring task":"Open delivery"}</button></li>;
}
