// Only the visible selected invocation owns an output stream; closing it never controls an agent.
import { useEffect } from "react";
import { url, type Endpoint } from "../api";
import { open } from "../streams";
import { useStore } from "../store";
import { childScope, type ChildDetail } from "./model";
import { patchChildOutput } from "./state";
import { RETRY_FIRST, RETRY_MAX, RETRY_FACTOR } from "./constants";

/** Injectable ownership checks reject late batches after parent, provider, checkout, or endpoint replacement. */
export function observeChildOutput(scope: string, id: string, ep: Endpoint, parent: string, agent: string, wt: string | undefined, current: () => boolean): () => void {
  let alive = true, stream: ReturnType<typeof open> | undefined, retry: ReturnType<typeof setTimeout> | undefined, delay = RETRY_FIRST;
  const connect = () => {
    if (!alive || !current()) return;
    stream = open(url(ep, "/api/session/subagent/output", { parent, invocation: id, agent, wt }), ep.token, (events) => {
      if (!alive || !current()) return;
      for (const event of events) {
        if (event.event === "output") {
          const detail = event.data as ChildDetail;
          if (!detail || !Array.isArray(detail.items)) continue;
          if (detail.availability === "disconnected") patchChildOutput(scope, id, { disconnected: true });
          else patchChildOutput(scope, id, { detail, disconnected: false });
          delay = RETRY_FIRST;
        } else if (event.event === "unavailable") patchChildOutput(scope, id, { disconnected: true });
      }
    }, () => {
      if (!alive || !current()) return;
      patchChildOutput(scope, id, { disconnected: true });
      retry = setTimeout(connect, delay); delay = Math.min(RETRY_MAX, delay * RETRY_FACTOR);
    });
  };
  connect();
  return () => { alive = false; clearTimeout(retry); stream?.close(); };
}

/** Parent input and socket ownership are unaffected by selection, visibility, collapse, or retries. */
export function useChildOutput(lane: string, scope: string, id: string | undefined, ep: Endpoint | undefined, enabled: boolean): void {
  const port = ep?.port, token = ep?.token;
  useEffect(() => {
    const source = useStore.getState().lanes[lane];
    if (!enabled || !id || !port || !token || !source?.session || childScope(source) !== scope) return;
    const endpoint = { port, token };
    return observeChildOutput(scope, id, endpoint, source.session, source.agent, source.wt, () => {
      const state = useStore.getState(), current = state.lanes[lane], endpoint = current && state.projects[current.project]?.endpoint;
      return childScope(current) === scope && endpoint?.port === port && endpoint.token === token;
    });
  }, [lane, scope, id, port, token, enabled]);
}
