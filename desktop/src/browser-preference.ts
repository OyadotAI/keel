// Device browser preference distinguishes saved defaults from the current session's binding.
import { useCallback, useEffect, useState } from "react";
import { get, post, type Endpoint } from "./api";
import { projectScratch } from "./project-creation";
import { useStore } from "./store";

export type BrowserBackend = "embedded" | "local_oya";
/** The card that asks where a lane browses (`approve::BROWSER_CHOICE`), held on the agent's first browser step. */
export const BROWSER_CHOICE = "BrowserChoice";
export interface BrowserPreference {
  selected: BrowserBackend | null; revision: number;
  installation: { application: string; executable: string } | null;
  detection_error: string | null; connection_state: string; connection_message: string;
  /** The browser the lane's next step uses — `null` when it hasn't chosen and will ask — and whose choice that is. */
  effective: { backend: BrowserBackend | null; source: "lane" | "default" | "none"; url: string | null; busy: boolean };
}
/** Loading never opens a browser or silently changes the selected backend. */
export function useBrowserPreference(lane?: string) {
  const project = useStore((s) => lane ? s.lanes[lane]?.project : s.active ? s.lanes[s.active]?.project : s.order[0]);
  const revision = useStore((s) => lane ? s.lanes[lane]?.browserRevision : undefined);
  const endpoint = useStore((s) => project ? s.projects[project]?.endpoint : undefined);
  const [scratch, setScratch] = useState<Endpoint>();
  const [value, setValue] = useState<BrowserPreference>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
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
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }, [ep, lane]);
  useEffect(() => { void refresh(); }, [refresh, revision]);
  /** The device setting: `null` is "Ask each lane", otherwise "Always …". */
  async function save(selected: BrowserBackend | null) {
    if (!ep || !value) return;
    setBusy(true); setError("");
    try { await post(ep, "/api/browser/preference", { selected, revision: value.revision }); await refresh(); }
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
  /** The Browser tab's "Browse in" switch: this lane only, from its next browser step. */
  async function setLane(backend: BrowserBackend) {
    if (!ep || !lane) return;
    setBusy(true); setError("");
    try {
      await post(ep, "/api/browser/lane", { backend }, { lane });
      useStore.setState((s) => (s.lanes[lane] ? { lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], browserInstall: undefined } } } : {}));
      await refresh();
    }
    catch (e) { setError(`Couldn’t change this lane’s browser. ${String(e)}`); }
    finally { setBusy(false); }
  }
  /** Open a page in the person's own Oya Browser, once — the lane keeps browsing where it was. */
  async function openInOya(url: string) {
    if (!ep) return;
    try { await post(ep, "/api/open-url", { url, oya: true }); }
    catch (e) { setError(`Couldn’t open it in Oya Browser. ${String(e)}`); }
  }
  return { reconnect, value, error, busy, save, setLane, openInOya, show, refresh };
}
