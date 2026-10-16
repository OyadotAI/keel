// Occupancy is independent of session billing and of estimates for files on disk.
import type { ContextSnapshot, ContextCategory } from "../context";
import { count } from "./Turn";

function Totals({ used, capacity, percent, busy }: { used?: number | null; capacity?: number | null; percent?: number; busy: boolean }) {
  const headline = percent != null ? `${percent}%` : used != null ? count(used) : "—";
  const label = percent != null ? "of context used" : used != null ? "context tokens" : busy ? "Reading context…" : "No usage reported yet";
  const detail = used != null && capacity ? `${count(used)} / ${count(capacity)} tokens` : "Capacity not reported";
  return <div className="context-total"><strong>{headline}</strong><div><span>{label}</span><span className="context-total-count">{detail}</span></div></div>;
}

function Meter({ percent, categories, capacity }: { percent?: number; categories: ContextCategory[]; capacity?: number | null }) {
  const valid = percent != null;
  return <div className={`context-meter${valid && percent >= 90 ? " near-limit" : ""}`} role={valid ? "progressbar" : undefined} aria-label="Context window occupancy" aria-valuemin={0} aria-valuemax={100} aria-valuenow={valid ? Math.min(100, percent) : undefined} aria-valuetext={valid ? `${percent}% used` : undefined}>
    {categories.length && capacity ? categories.filter((c) => c.kind !== "deferred").map((c, i) => <span key={`${c.name}:${i}`} className={`context-segment context-tone-${i % 6} ${c.kind}`} style={{ width: `${Math.min(100, c.tokens / capacity * 100)}%` }} title={`${c.name}: ${count(c.tokens)} tokens`} />) : <span className="context-segment" style={{ width: valid ? `${Math.min(100, percent)}%` : "0%" }} />}
  </div>;
}

function Breakdown({ snapshot }: { snapshot: ContextSnapshot }) {
  const categories = snapshot.categories.filter((c) => c.tokens > 0);
  if (!categories.length) return <p className="context-note">A category breakdown has not been reported. File estimates below are separate from the usage total.</p>;
  return <div className={`context-breakdown${snapshot.categories_stale ? " stale" : ""}`}>
    {snapshot.categories_stale && <p className="context-note">Last breakdown · {snapshot.breakdown_at ? new Date(snapshot.breakdown_at).toLocaleTimeString() : "time not reported"}. Refresh for current estimates.</p>}
    {categories.map((c, i) => <div className="context-category" key={`${c.name}:${i}`}><span className={`context-swatch context-tone-${i % 6} ${c.kind}`} /><span>{c.name}{c.kind === "deferred" && <small> outside context</small>}</span><span className="context-number">{count(c.tokens)}</span><span className="context-percent">{snapshot.capacity && c.kind !== "deferred" ? `${(c.tokens / snapshot.capacity * 100).toFixed(1)}%` : "—"}</span></div>)}
  </div>;
}

export function ContextUsage({ snapshot, busy, disconnected }: { snapshot?: ContextSnapshot; busy: boolean; disconnected: boolean }) {
  const used = snapshot?.used;
  const capacity = snapshot?.capacity;
  const percent = used != null && capacity ? Math.round(used / capacity * 100) : undefined;
  const stale = disconnected || snapshot?.origin !== "runtime";
  const updated = snapshot?.updated_at ? `${stale ? "Saved" : "Updated"} ${new Date(snapshot.updated_at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}` : "Waiting for provider";
  const categories = snapshot && !snapshot.categories_stale ? snapshot.categories.filter((c) => c.tokens > 0) : [];
  return <section className="context-occupancy" aria-label="Context window usage">
    <Totals used={used} capacity={capacity} percent={percent} busy={busy} />
    <Meter percent={percent} categories={categories} capacity={capacity} />
    <div className="context-caption"><span>{snapshot?.usage_accuracy === "provider_estimate" ? "Provider estimate" : used == null ? "No measurement" : "Provider-reported usage"}</span><span>{updated}</span></div>
    {snapshot?.window_kind === "compaction_window" && <p className="context-note">Measured against the provider’s compaction window.</p>}
    {percent != null && percent > 100 && <p className="context-error">Reported usage exceeds this window. The provider determines whether compaction or a limit applies.</p>}
    {snapshot && <Breakdown snapshot={snapshot} />}
  </section>;
}
