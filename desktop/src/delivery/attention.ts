// Stable task identities deduplicate handoff aliases while pending questions retain priority.
import type { TaskPage, SchedulePage, TaskSummary } from "./model";
import {ATTENTION_PRIORITY as PRIORITY} from "./constants";
/** Persisted reasons survive closed lanes and can open the exact task or recurring definition. */
export interface SavedAttention { id: string; task?: string; title: string; lanes: string[]; reasons: string[]; recurring: boolean; priority:number }
/** Missing reads do not manufacture a healthy empty list. */
export function savedAttention(tasks?: TaskPage, schedules?: SchedulePage): SavedAttention[] {
  const items: SavedAttention[] = [];
  for(const task of tasks?.tasks??[]){const item=taskAttention(task);if(item)items.push(item);}
  for (const schedule of schedules?.schedules ?? []) {
    const active = (schedules?.active_occurrence??schedules?.occurrences.find((o) => o.id === schedules.active))?.schedule===schedule.id?(schedules?.active_occurrence??schedules?.occurrences.find(o=>o.id===schedules.active)):undefined;
    const reasons = [schedule.error, active && ["unconfirmed", "stopping", "interrupted"].includes(active.phase) ? `Active run: ${active.phase}` : undefined].filter((reason): reason is string => !!reason);
    if (reasons.length) items.push({ id: `schedule:${schedule.id}`, title: schedule.name, lanes: [], reasons, recurring: true,priority:PRIORITY.error });
  }
  if(schedules?.controller_error)items.push({id:"dispatcher",title:"Recurring dispatch paused",lanes:[],reasons:[schedules.controller_error],recurring:true,priority:PRIORITY.error});
  return items;
}

function taskAttention(task:TaskSummary):SavedAttention|undefined {
    const reasons: string[] = [];
    let priority:number=PRIORITY.correction;
    if (task.unresolved) reasons.push(`${task.unresolved} unresolved correction${task.unresolved === 1 ? "" : "s"}`);
    if (task.check && ["failed", "aborted", "running"].includes(task.check.status)) {reasons.unshift(`Last recorded checks: ${task.check.status}`);priority=PRIORITY.checks;}
    if (task.publication && (task.publication.status !== "confirmed" || task.publication.actual_head !== task.publication.intent.identity.head)) {reasons.unshift("Publication needs inspection");priority=PRIORITY.error;}
    return reasons.length ? { id: task.task, task: task.task, title: task.title || "Conversation task", lanes: task.lanes ?? [task.owner.lane], reasons, recurring: false,priority } : undefined;
}
