// New-project effects retain successful creation separately from opening and release scratch daemons.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { type Endpoint } from "./api";
import { useStore } from "./store";

/** Temporary daemons exist only for the operation; late completion still releases them. */
export async function projectScratch(): Promise<{ ep: Endpoint; done: () => void }> {
  const dir = await invoke<string>("scratch_dir");
  const ep = await invoke<Endpoint>("open_project", { path: dir });
  return { ep, done: () => void invoke("close_project", { path: dir }) };
}
/** Opening failure keeps the actual created path and never issues a second creation request. */
export function useProjectCreation() {
  const [parent, setParent] = useState<string>();
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();
  const [created, setCreated] = useState<string>();
  const alive = useRef(true);
  const operation = useRef(crypto.randomUUID());
  useEffect(() => {
    alive.current = true;
    void invoke<string>("projects_dir").then((path) => { if (alive.current) setParent((current) => current ?? path); }, () => { if (alive.current) setError("Couldn’t access the Keel projects folder. Choose another location."); });
    return () => { alive.current = false; };
  }, []);
  async function open(path: string) {
    await invoke<Endpoint>("open_project", { path });
    if (!alive.current) { void invoke("close_project", { path }); return; }
    await useStore.getState().openProject(path);
    useStore.setState({ creating: false });
  }
  async function run(what: string, act: (ep: Endpoint, operation: string) => Promise<string>) {
    if (!parent || busy) return;
    setBusy(what); setError(undefined);
    let done = () => {};
    try {
      const scratch = await projectScratch(); done = scratch.done;
      if (!alive.current) return;
      const path = await act(scratch.ep, operation.current);
      if (!alive.current) return;
      setCreated(path);
      try { await open(path); } catch (e) { throw new Error(`Project created at ${path}, but Keel couldn’t open it. ${String(e)}`); }
    } catch (e) { if (alive.current) setError(String(e)); }
    finally { done(); if (alive.current) setBusy(undefined); }
  }
  async function reopen() {
    if (!created || busy) return;
    setBusy("open"); setError(undefined);
    try { await open(created); } catch (e) { if (alive.current) setError(`Couldn’t open ${created}: ${String(e)}`); }
    finally { if (alive.current) setBusy(undefined); }
  }
  return { parent, setParent, busy, error, setError, created, run, reopen, resetOperation: () => { operation.current = crypto.randomUUID(); } };
}

/** Preview the effective path using the resolved platform’s separators without reserving a name. */
export function projectDestination(parent: string, name: string): string {
  const separator = parent.includes("\\") ? "\\" : "/";
  return `${parent.replace(/[\\/]$/, "")}${separator}${name.trim()}`;
}
