// Device browser preference distinguishes saved defaults from the current session's binding.
import { useCallback, useEffect, useState } from "react";
import { get, post, type Endpoint } from "./api";
import { projectScratch } from "./project-creation";
import { useStore } from "./store";

export type BrowserBackend = "embedded" | "local_oya";
export interface BrowserPreference {
  selected: BrowserBackend | null; revision: number;
  installation: { application: string; executable: string } | null;
  detection_error: string | null; connection_state: string; connection_message: string;
  effective: { backend: BrowserBackend; url: string | null; busy: boolean };
}
let deferred = false;
/** Loading never opens a browser or silently changes the selected backend. */
export function useBrowserPreference(lane?: string) {
  const project = useStore((s) => lane ? s.lanes[lane]?.project : s.active ? s.lanes[s.active]?.project : s.order[0]);
  const revision = useStore((s) => lane ? s.lanes[lane]?.browserRevision : undefined);
  const endpoint = useStore((s) => project ? s.projects[project]?.endpoint : undefined);
  const [scratch, setScratch] = useState<Endpoint>();
  const [value, setValue] = useState<BrowserPreference>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [choosing, setChoosing] = useState(false);
  const ep = endpoint ?? scratch;
  useEffect(() => {
    if (project) { void useStore.getState().ensure(project).catch((e) => setError(String(e))); return; }
    let disposed = false; let done = () => {};
    void projectScratch().then((result) => { done = result.done; if (disposed) done(); else setScratch(result.ep); }, (e) => { if (!disposed) setError(String(e)); });
    return () => { disposed = true; done(); setScratch(undefined); };
  }, [project]);
  const refresh = useCallback(async () => {
    if (!ep) return;
    setBusy(true);
    try {
      const next = await get<BrowserPreference>(ep, "/api/browser/preference", { lane });
      setValue(next); setError("");
      if (!next.selected && next.installation && !deferred) setChoosing(true);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }, [ep, lane]);
  useEffect(() => { void refresh(); }, [refresh, revision]);
  async function save(selected: BrowserBackend) {
    if (!ep || !value) return;
    setBusy(true); setError("");
    try { await post(ep, "/api/browser/preference", { selected, revision: value.revision }); setChoosing(false); await refresh(); }
    catch (e) { setError(`Couldn’t save your browser choice. ${String(e)}`); }
    finally { setBusy(false); }
  }
  async function show() {
    if (!ep || !lane) return;
    setBusy(true); setError("");
    try { await post(ep, "/api/browser/show-local", {}, { lane }); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function reconnect() {
    if (!ep || !lane) return;
    setBusy(true); setError("");
    try { await post(ep, "/api/browser/reconnect", {}, { lane }); await refresh(); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  return { reconnect, value, error, busy, choosing, choose: () => setChoosing(true), defer: () => { deferred = true; setChoosing(false); }, save, show, refresh };
}
