// Endpoint-scoped snapshots refresh on daemon events and reject superseded reads.
import { useEffect, useSyncExternalStore } from "react";
import { get, type Endpoint } from "../api";
import type { TaskView, TaskPage, SchedulePage } from "./model";
interface Snapshot { view?: TaskView | null; tasks?: TaskPage; schedules?: SchedulePage; error?: string; loading: boolean }
const snapshots = new Map<string, Snapshot>(), listeners = new Set<() => void>(), tickets = new Map<string, number>();
const bindings = new Map<string, { ep: Endpoint; lane?: string }>();
let revision = 0;
const keyOf = (ep: Endpoint, lane?: string) => JSON.stringify([ep.port, ep.token, lane]);
const emit = () => { revision++; listeners.forEach((listener) => listener()); };
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
const EMPTY: Snapshot = { loading: true };

/** Lost reads retain history while revoking current evidence and mutation authority in the view. */
export function observedTask(snapshot: {view?:TaskView|null;error?:string}) {
  return snapshot.view && snapshot.error ? {...snapshot.view,checks_current:false,checks_passed:false,control_error:snapshot.error} : snapshot.view;
}

/** Read failure preserves useful old facts while revoking confirmation. */
export async function refreshDelivery(ep: Endpoint, lane?: string) {
  const key = keyOf(ep, lane), ticket = (tickets.get(key) ?? 0) + 1;
  tickets.set(key, ticket); bindings.set(key, { ep, lane });
  snapshots.set(key, { ...snapshots.get(key), loading: true }); emit();
  try {
    const patch = lane ? { view: await get<TaskView | null>(ep, "/api/delivery/by-lane", { lane }) } : await project(ep);
    if (tickets.get(key) !== ticket) return;
    const confirmed = Object.fromEntries(Object.entries(patch).filter(([, value]) => value !== undefined));
    snapshots.set(key, { ...snapshots.get(key), ...confirmed, error: "error" in patch ? patch.error : undefined, loading: false });
  } catch (error) { if (tickets.get(key) === ticket) snapshots.set(key, { ...snapshots.get(key), loading: false, error: String(error) }); }
  emit();
}
async function project(ep: Endpoint) {
  const [tasks, schedules] = await Promise.allSettled([get<TaskPage>(ep, "/api/tasks"), get<SchedulePage>(ep, "/api/delivery/schedules")]);
  return { tasks: tasks.status === "fulfilled" ? tasks.value : undefined, schedules: schedules.status === "fulfilled" ? schedules.value : undefined, error: [tasks, schedules].flatMap((result) => result.status === "rejected" ? [String(result.reason)] : []).join(" · ") || undefined };
}
/** Attention reads the same event-backed project discovery without duplicating requests. */
export const projectDelivery = (ep: Endpoint) => snapshots.get(keyOf(ep));
/** A primitive revision keeps aggregate projections stable between observed delivery events. */
export function useDeliveryRevision() { return useSyncExternalStore(subscribe, () => revision, () => revision); }
/** Events update observed contexts only; this never polls GitHub. */
export function deliveryChanged(ep: Endpoint) {
  for (const binding of bindings.values()) if (binding.ep.port === ep.port && binding.ep.token === ep.token) void refreshDelivery(ep, binding.lane);
}
/** Both layouts share the same current task snapshot. */
export function useDelivery(ep?: Endpoint, lane?: string) {
  const key = ep && keyOf(ep, lane);
  useEffect(() => { if (ep) void refreshDelivery(ep, lane); }, [ep, lane]);
  const snapshot = () => key ? snapshots.get(key) ?? EMPTY : EMPTY;
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}
