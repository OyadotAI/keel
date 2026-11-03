// Inspection and explicit continuation preserve the same receipt when a launch response is lost.
import { useCallback, useEffect, useRef, useState } from "react";
import { get, post } from "../api";
import { useStore, type Agent } from "../store";
import type { Continuation, TaskView } from "../handoffs";

/** Loading task notes is independent of provider context inspection and never starts a model. */
export function useHandoff(lane: string) {
  const l = useStore((s) => s.lanes[lane]);
  const ep = useStore((s) => l && s.projects[l.project]?.endpoint);
  const [view, setView] = useState<TaskView>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState("");
  const [provider, setProvider] = useState<Agent | "">("");
  const key = `${l?.project}:${l?.agent}:${l?.session}`;
  const scope = useRef(key); scope.current = key;
  const receipt = useRef<{ id: string; revision: number; provider: Agent } | undefined>(undefined);
  const identity = { lane, provider: l?.agent ?? "claude", session: l?.session ?? "" };
  const refresh = useCallback(async (clearError = true) => {
    if (!ep || !l?.session) return;
    const captured = key;
    try { const next = await get<TaskView>(ep, "/api/task", { lane, provider: l.agent, session: l.session }); if (scope.current === captured) { setView(next); if (clearError) setError(""); } }
    catch (e) { if (scope.current === captured) setError(String(e)); }
  }, [ep, key, l?.agent, l?.session, lane]);
  useEffect(() => { setView(undefined); receipt.current = undefined; void refresh(); }, [refresh]);
  async function act(label: string, action: () => Promise<void>) {
    const captured = key; setBusy(label); setError("");
    try { await action(); } catch (e) { if (scope.current === captured) setError(String(e)); }
    finally { if (scope.current === captured) { setBusy(""); await refresh(false); } }
  }
  const save = () => act("Saving handoff…", async () => { if (ep) await post(ep, "/api/task/save", identity); });
  const stop = () => act("Stopping source work…", async () => {
    if (!ep || !view?.task) return;
    await post(ep, "/api/task/stop", identity);
  });
  const proceed = () => act(`Starting ${provider}…`, async () => {
    if (!ep || !provider || !view?.saved || !l) return;
    const captured = key;
    if (!receipt.current || receipt.current.revision !== view.saved.revision || receipt.current.provider !== provider) receipt.current = { id: crypto.randomUUID(), revision: view.saved.revision, provider };
    const next = await post<Continuation>(ep, "/api/task/continue", { source: identity, revision: receipt.current.revision, provider: receipt.current.provider, receipt: receipt.current.id });
    if (scope.current !== captured) return;
    const destination = useStore.getState().adoptContinuation(lane, next);
    const accepted = await useStore.getState().send(destination, next.prompt);
    if (!accepted) throw new Error("Couldn’t confirm the destination started. Open the continuation lane and retry its saved submission; your handoff is available.");
  });
  const recover = () => act("Loading continuation…", async () => {
    const continuation = view?.task?.continuation;
    if (!continuation) return;
    const destination = useStore.getState().adoptContinuation(lane, continuation);
    if (useStore.getState().lanes[destination]?.receipt?.id === continuation.id) await useStore.getState().send(destination, continuation.prompt);
  });
  return { recover, view, error, busy, provider, setProvider, save, stop, proceed, refresh, lane: l };
}
