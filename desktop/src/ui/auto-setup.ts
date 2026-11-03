// In a trusted project Keel adds and updates its own team on its own; this says how that went.
import { post, type Endpoint } from "../api";
import { useStore } from "../store";

/// What a card that used to carry an install button shows instead: nothing to do, or a failed
/// attempt with a retry. `undefined` in an untrusted project, where the button stays.
export function useAutoSetup(project: string | undefined, ep: Endpoint | undefined): { failed?: string | null; retry: () => void } | undefined {
  const trusted = useStore((s) => !!project && !!s.trusted[project]);
  const record = useStore((s) => (project ? s.setupRun[project] : undefined));
  if (!trusted) return undefined;
  return { failed: record?.phase === "failed" ? record.error : undefined, retry: () => { if (ep) void post(ep, "/api/setup/run").catch(() => undefined); } };
}
