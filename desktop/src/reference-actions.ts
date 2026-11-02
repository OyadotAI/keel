// Validate checkout-bound draft references before presenting them as usable context.
import { post, type Endpoint } from "./api";
import { sameCheckout, sameReference, type FileReference } from "./chat-files";
import { requestChatFocus } from "./chat-focus";
import type { Lane } from "./store";
import { MAX_FILE_REFERENCES } from "./chat-constants";

interface Dependencies {
  read(): { lanes: Record<string, Lane> };
  patch(id: string, patch: Partial<Lane> | ((lane: Lane) => Partial<Lane>)): void;
  endpoint(project: string): Promise<Endpoint>;
  persist(): boolean;
}

function available(lane: Lane | undefined, reference: FileReference): string | undefined {
  if (!lane || lane.interface === "terminal") return "Open this file in a chat conversation.";
  if (!sameCheckout(lane, reference)) return "This file belongs to another checkout.";
  if (lane.isolated && !lane.wt) return "This lane’s checkout is not available yet.";
  if (lane.disconnected) return "Reconnect to check this file reference.";
  if ((lane.references?.length ?? 0) >= MAX_FILE_REFERENCES && !lane.references?.some((r) => sameReference(r, reference))) return "Remove a reference before adding another file.";
}

/** A single ownership boundary prevents delayed validation from attaching to another checkout. */
export function createReferenceActions(deps: Dependencies) {
  async function referenceFile(id: string, reference: FileReference): Promise<boolean> {
    const lane = deps.read().lanes[id];
    const error = available(lane, reference);
    if (error) { deps.patch(id, { referenceError: error }); return false; }
    deps.patch(id, (l) => ({ referencePending: (l.referencePending ?? 0) + 1, referenceError: undefined, referenceStatus: "Checking file reference…" }));
    try {
      const endpoint = await deps.endpoint(lane.project);
      await post(endpoint, "/api/references/validate", { wt: lane.wt, reference });
      const current = deps.read().lanes[id];
      const stale = available(current, reference);
      if (stale) throw new Error(stale);
      const duplicate = current.references?.some((r) => sameReference(r, reference));
      deps.patch(id, (l) => ({ references: duplicate ? l.references : [...(l.references ?? []), reference], referenceStatus: duplicate ? "File already referenced." : "File referenced." }));
      deps.persist(); requestChatFocus(id); return true;
    } catch (error) {
      deps.patch(id, { referenceError: `Could not reference file: ${error}`, referenceStatus: undefined });
      return false;
    } finally { deps.patch(id, (l) => ({ referencePending: Math.max(0, (l.referencePending ?? 1) - 1) })); }
  }
  function removeReference(id: string, reference: FileReference): void {
    deps.patch(id, (l) => ({ references: l.references?.filter((r) => !sameReference(r, reference)), referenceStatus: undefined }));
    deps.persist();
  }
  return { referenceFile, removeReference };
}
