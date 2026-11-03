// Single-flight batch reads avoid consuming one browser SSE slot per child.
import { useEffect } from "react";
import { url, type Endpoint } from "../api";
import { useStore } from "../store";
import { childScope, type ChildDetail } from "./model";
import { patchChildOutput } from "./state";
import { GRID_BATCH, GRID_POLL_MS, GRID_REQUEST_MS, RETRY_FIRST, RETRY_FACTOR, RETRY_MAX } from "./constants";
export interface GridSource { parent: string; agent: string; wt?: string }
/** Rotation gives every visible invocation a read beyond the first batch. */
export function outputBatch(ids: string[], cursor: number): string[] {
  return Array.from({ length: Math.min(ids.length, GRID_BATCH) }, (_, index) => ids[(cursor + index) % ids.length]);
}
/** Cleanup aborts observation and revokes late responses without touching providers. */
export function observeGridOutput(scope: string, ids: string[], ep: Endpoint, source: GridSource, current: () => boolean, request = fetch): () => void {
  let alive = true, cursor = 0, delay = RETRY_FIRST, retry: ReturnType<typeof setTimeout> | undefined;
  let controller: AbortController | undefined;
  const poll = async () => {
    if (!alive || !current() || !ids.length) return;
    const batch = outputBatch(ids, cursor);
    cursor = (cursor + batch.length) % ids.length;
    controller = new AbortController();
    const timeout = setTimeout(() => controller?.abort(), GRID_REQUEST_MS);
    let next = GRID_POLL_MS;
    try {
      const response = await request(url(ep, "/api/session/subagents/output", { wt: source.wt }), {
        method: "POST", headers: { Authorization: "Bearer " + ep.token, "Content-Type": "application/json" },
        body: JSON.stringify({ parent: source.parent, agent: source.agent, invocations: batch }), signal: controller.signal,
      });
      if (!response.ok) throw new Error("Subagent output is unavailable.");
      const details = await response.json() as Record<string, ChildDetail>;
      if (!alive || !current()) return;
      for (const id of batch) {
        const detail = details[id];
        if (detail && Array.isArray(detail.items)) patchChildOutput(scope, id, { detail, disconnected: detail.availability === "disconnected" });
        else patchChildOutput(scope, id, { disconnected: true });
      }
      delay = RETRY_FIRST;
    } catch {
      if (!alive || !current()) return;
      for (const id of batch) patchChildOutput(scope, id, { disconnected: true });
      next = delay; delay = Math.min(RETRY_MAX, delay * RETRY_FACTOR);
    } finally {
      clearTimeout(timeout);
      if (alive && current()) retry = setTimeout(() => void poll(), next);
    }
  };
  void poll();
  return () => { alive = false; clearTimeout(retry); controller?.abort(); };
}
/** Hidden lanes and replaced endpoints cannot keep updating a visible grid. */
export function useGridOutput(lane: string, scope: string, ids: string[], ep?: Endpoint): void {
  const key = JSON.stringify(ids), port = ep?.port, token = ep?.token;
  useEffect(() => {
    const source = useStore.getState().lanes[lane];
    const selected = JSON.parse(key) as string[];
    if (!selected.length || !source?.session || !port || !token || childScope(source) !== scope) return;
    return observeGridOutput(scope, selected, { port, token }, { parent: source.session, agent: source.agent ?? "claude", wt: source.wt }, () => {
      const state = useStore.getState(), current = state.lanes[lane], endpoint = current && state.projects[current.project]?.endpoint;
      return childScope(current) === scope && endpoint?.port === port && endpoint.token === token;
    });
  }, [lane, scope, key, port, token]);
}
