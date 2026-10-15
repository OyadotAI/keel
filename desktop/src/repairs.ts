import { useStore } from "./store";

/** Project repairs need the shared checkout and an editing lane; keep planning lanes intact. */
export function repairLane(project: string, title = "Readiness fixes"): string {
  const store = useStore.getState();
  const usable = (id?: string) => {
    const lane = id ? store.lanes[id] : undefined;
    return !!lane && lane.project === project && !lane.isolated && lane.interface === "chat" && lane.mode === "acceptEdits";
  };
  if (usable(store.active)) return store.active!;
  const existing = store.projects[project]?.lanes.find((id) => usable(id));
  if (existing) return existing;
  const id = store.newLane(project, false, "claude", "chat");
  useStore.setState((s) => ({ lanes: { ...s.lanes, [id]: { ...s.lanes[id], title, mode: "acceptEdits" } } }));
  return id;
}
