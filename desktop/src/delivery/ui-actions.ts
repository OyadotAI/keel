// Task navigation recovers the current owner and uses the existing workspace surface.
import { recoverOwner } from "./actions";
import type { Endpoint } from "../api";
/** Opening a durable task selects its original checkout and starts no provider. */
export async function openTask(project: string, ep: Endpoint, task: string, title: string) {
  const owner = await recoverOwner(ep, task);
  const { useStore } = await import("../store");
  useStore.getState().adoptTask(project, owner, title);
  useStore.setState({ panelTab: "review" });
}
