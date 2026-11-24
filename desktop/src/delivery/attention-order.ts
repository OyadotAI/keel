// Live lanes and saved tasks share priorities while task aliases retain every unresolved cause.
import type {AttentionItem} from "../attention";
import type {SavedAttention} from "./attention";
import {ATTENTION_PRIORITY as PRIORITY} from "./constants";
type Saved=SavedAttention&{project:string};
/** A discriminated row retains the existing live destination or the exact durable task action. */
export type SupervisionRow<S extends Saved>={kind:"live";item:AttentionItem;priority:number}|{kind:"saved";item:S;priority:number};
const livePriority=(item:AttentionItem)=>PRIORITY[item.reasons[0]?.kind??"update"];
/** Pending input wins; grouped errors/checks remain visible, corrections precede unread endings. */
export function orderAttention<S extends Saved>(live:AttentionItem[],saved:S[],projects:readonly string[]):SupervisionRow<S>[] {
  const consumed=new Set<AttentionItem>();
  const rows:SupervisionRow<S>[]=saved.filter(task=>!live.some(item=>task.project===item.project&&task.lanes.includes(item.lane)&&item.reasons.some(reason=>reason.kind==="input"))).map(task=>{
    const matches=live.filter(item=>task.project===item.project&&task.lanes.includes(item.lane));
    matches.forEach(item=>consumed.add(item));
    const primary=[...matches].sort((a,b)=>livePriority(a)-livePriority(b))[0];
    if(primary&&livePriority(primary)<=task.priority) {
      const reasons=[...primary.reasons,...task.reasons.map(text=>({kind:"update" as const,text})),...matches.filter(item=>item!==primary).flatMap(item=>item.reasons)];
      return {kind:"live",item:{...primary,reasons},priority:livePriority(primary)};
    }
    return {kind:"saved",item:{...task,reasons:[...task.reasons,...matches.flatMap(item=>item.reasons.map(reason=>reason.text))]},priority:task.priority};
  });
  rows.push(...live.filter(item=>!consumed.has(item)).map(item=>({kind:"live" as const,item,priority:livePriority(item)})));
  return rows.sort((a,b)=>a.priority-b.priority||projects.indexOf(a.item.project)-projects.indexOf(b.item.project));
}
