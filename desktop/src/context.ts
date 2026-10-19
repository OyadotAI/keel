// Provider-neutral context metadata. Counts absent from a provider stay absent, never zero-filled.
export interface ContextCategory { name: string; tokens: number; kind: "used" | "free" | "buffer" | "deferred"; accuracy?: string }
export interface ContextSource { path: string; source?: string; state?: string; tokens?: number; estimate?: number; accuracy?: string }
export interface ContextSkill { name: string; path?: string; description?: string; source?: string; available?: boolean; advertised?: boolean; tokens?: number; estimate?: number; user_only?: boolean }
export interface ContextEvidence { key: string; id?: string; kind: string; name?: string; path?: string; at?: string; turn?: string; member?: string; detail?: string }
export interface ContextSnapshot {
  provider: "claude" | "codex" | "antigravity";
  session: string;
  updated_at: string;
  origin: string;
  model?: string | null;
  used?: number | null;
  capacity?: number | null;
  window_kind?: string | null;
  usage_accuracy?: string | null;
  categories: ContextCategory[];
  categories_stale?: boolean;
  breakdown_at?: string | null;
  reserve?: number | null;
  memory: ContextSource[];
  skills: ContextSkill[];
  evidence: ContextEvidence[];
  compacted_at?: string | null;
  limitations: string[];
}

export function contextMatches(snapshot: ContextSnapshot | undefined, provider: string, session?: string): snapshot is ContextSnapshot {
  return !!snapshot && snapshot.provider === provider && snapshot.session === (session ?? "");
}

export const evidenceLabel = (kind: string) => ({ invocation: "Invoked", failed_invocation: "Invocation failed", read: "Read observed", requested: "Requested", preload: "Declared preload", memory_read: "Read observed", memory_recall: "Recalled" })[kind] ?? kind;
