// A validated drop belongs to one live terminal connection and must never become queued input.
import { HttpError, post, type Endpoint } from "./api";
import { sameCheckout, type FileReference } from "./chat-files";
import { pasteInto, terminalLease } from "./typers";
import { terminalFilePath } from "./terminal-file-path";
import type { Lane } from "./store";

interface Snapshot { lanes: Record<string, Lane>; active?: string }
/** Composition supplies checkout validation and lifecycle observation without owning a terminal. */
export interface TerminalReferenceDependencies {
  read(): Snapshot;
  patch(id: string, patch: Partial<Lane> | ((lane: Lane) => Partial<Lane>)): void;
  endpoint(project: string): Promise<Endpoint>;
  subscribe(listener: (state: Snapshot) => void): () => void;
}
const CHANGED = "The destination changed. Drop the file into the intended terminal again.";
const NOT_READY = "Terminal isn’t ready. Reconnect or start it, then drop the file again.";
const identity = (lane?: Lane) => lane && JSON.stringify([lane.project, lane.wt, lane.agent, lane.session, lane.interface]);

/** Known decision pickers are never treated as composers; unavailable targets explain rejection. */
export function terminalReferenceProblem(lane: Lane | undefined, active: string | undefined, reference?: FileReference): string | undefined {
  if (!lane || lane.interface !== "terminal" || active !== lane.id) return CHANGED;
  if (lane.elsewhere) return "Open this session here before inserting a file.";
  if (reference && !sameCheckout(lane, reference)) return "This file belongs to another checkout.";
  if (lane.pending.length || lane.attention) return "Answer the current question or approval before dropping a file.";
  if (lane.disconnected || !lane.session || (lane.isolated && !lane.wt) || !terminalLease(lane.id)) return NOT_READY;
}

/** Only a still-visible lease may paste after metadata validation; no failure schedules a retry. */
export async function insertTerminalFile(deps: TerminalReferenceDependencies, id: string, reference: FileReference): Promise<boolean> {
  const state = deps.read(), original = state.lanes[id];
  if (original?.referencePending) return false;
  const problem = terminalReferenceProblem(original, state.active, reference);
  if (problem) { deps.patch(id, { referenceError: problem, referenceStatus: undefined }); return false; }
  const lease = terminalLease(id)!, captured = identity(original);
  let changed = false;
  const unwatch = deps.subscribe((s) => { if (s.active !== id || identity(s.lanes[id]) !== captured || !lease.current()) changed = true; });
  deps.patch(id, { referencePending: 1, referenceError: undefined, referenceStatus: `Checking file… ${reference.path}` });
  try {
    const text = terminalFilePath(reference.path);
    const endpoint = await deps.endpoint(original.project);
    if (changed) throw new Error(CHANGED);
    const result = await post<{ ok: boolean }>(endpoint, "/api/references/validate", { wt: original.wt, reference });
    if (!result?.ok) throw new Error("This file is no longer available in this checkout.");
    const current = deps.read(), invalid = terminalReferenceProblem(current.lanes[id], current.active, reference);
    if (changed || identity(current.lanes[id]) !== captured || !lease.current()) throw new Error(CHANGED);
    if (invalid) throw new Error(invalid);
    if (!pasteInto(id, text, lease)) throw new Error(NOT_READY);
    deps.patch(id, { referenceStatus: `File path inserted. Continue typing when ready. ${reference.path}` });
    lease.focus();
    return true;
  } catch (error) {
    const message = error instanceof HttpError && error.status === 400 ? "This file is no longer available in this checkout." : error instanceof Error ? error.message : String(error);
    deps.patch(id, { referenceError: message, referenceStatus: undefined }); return false;
  } finally { unwatch(); deps.patch(id, { referencePending: 0 }); }
}
