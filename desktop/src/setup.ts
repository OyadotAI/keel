import { get, type Endpoint } from "./api";
import { offered, pending, readTeam } from "./team";

/// What the daemon's automatic setup is doing or last did for a project (`GET /api/setup`,
/// then `setup.changed`). In a trusted project this, not a window's own run, is what the chip
/// shows: two windows see one result, and a relaunch keeps it.
export interface SetupRecord {
  root: string;
  phase: "idle" | "running" | "waiting" | "done" | "failed";
  trigger: "open" | "click";
  step?: string | null;
  /// `turn`, `terminal`, `keel`, a git operation's name, or `you` (paths with your uncommitted edits).
  waiting?: { on: string; who?: string | null; paths: string[] } | null;
  team?: { kind: "added" | "updated"; files: string[]; kept: string[]; yours: string[]; commit?: string | null; note?: string | null } | null;
  plugins: { name: string; commit?: string | null; error?: string | null }[];
  error?: string | null;
  at: number;
}

export const readSetupRecord = (ep: Endpoint) => get<SetupRecord>(ep, "/api/setup");

/// Whether the record carries anything a person would want to see: a run, a wait, a result.
export const recordShown = (r?: SetupRecord): r is SetupRecord => !!r && r.phase !== "idle";

/// The warnings the automatic setup answers for; in a trusted project their rows read from the record.
export const automatic = (w: Warning) => w.id === "team/update" || w.id.startsWith("agent/") || w.id.startsWith("plugin/");

/// What the chip says, in what colour, and what a screen reader hears. A trusted project reads
/// the daemon's record; an untrusted one, or a daemon with no record, reads today's warnings.
export function chipState(record: SetupRecord | undefined, warnings: Warning[] | undefined, trusted: boolean, running: boolean): { label: string; color: string; aria: string } {
  const count = warnings?.length ?? 0;
  const critical = !!warnings?.some((w) => w.severity === "critical");
  const auto = trusted && recordShown(record) ? automaticChip(record, critical) : undefined;
  if (auto) return auto;
  const onlyUpdate = count === 1 && warnings![0].id === "team/update";
  const reviewOnly = count > 0 && warnings!.every((w) => /^security\/untrusted-agent-config$/.test(w.id));
  const label = running ? "Setting up…" : onlyUpdate ? "Team update" : count ? `${count} ${reviewOnly ? "to review" : "to set up"}` : "Setup result";
  return { label, color: critical ? "var(--del)" : onlyUpdate ? "var(--dim)" : "var(--warn)", aria: `${count} setup warnings` };
}

/// The automatic run's own states; a failure falls through to the warnings, which name it.
function automaticChip(record: SetupRecord, critical: boolean): { label: string; color: string; aria: string } | undefined {
  const quiet = critical ? "var(--del)" : "var(--dim)";
  if (record.phase === "running") return { label: "Setting up…", color: quiet, aria: "Setup: running" };
  if (record.phase === "waiting" && record.waiting?.on === "you") {
    const n = record.waiting.paths.length || 1;
    return { label: `${n} waiting on you`, color: critical ? "var(--del)" : "var(--warn)", aria: `Setup: ${n} item${n === 1 ? "" : "s"} waiting on you` };
  }
  if (record.phase === "waiting") {
    const on = record.waiting?.on;
    const after = on === "turn" || on === "terminal" || on === "keel" ? "this turn" : `the ${on ?? "merge"}`;
    return { label: `Setup · after ${after}`, color: quiet, aria: `Setup: waiting for ${after}` };
  }
  if (record.phase !== "done") return undefined;
  const commits = [record.team?.commit, ...record.plugins.map((p) => p.commit)].filter(Boolean).length;
  return { label: record.team?.kind === "updated" ? "Team updated" : "Set up", color: quiet, aria: `Setup: done, ${commits} commit${commits === 1 ? "" : "s"}` };
}

/// The result block's lines: what Keel did, and the commit each part landed in.
export function resultLines(record: SetupRecord): string[] {
  const out: string[] = [];
  const t = record.team;
  if (t && (t.files.length || t.commit)) {
    const head = t.kind === "updated" ? `Team updated · ${t.files.length} file${t.files.length === 1 ? "" : "s"}` : `Team added · ${t.files.length} file${t.files.length === 1 ? "" : "s"}`;
    out.push(`${head}${t.commit ? ` · commit ${t.commit}` : t.note ? ` · ${t.note}` : ""}${t.kept.length ? ` · kept ${t.kept.length} you changed` : ""}`);
  }
  for (const p of record.plugins) if (!p.error) out.push(`Plugin ${p.name} installed${p.commit ? ` · commit ${p.commit}` : ""}`);
  return out;
}

/// Something about how this project is set up for an agent that is missing or not right: the CLI
/// not installed or logged in, a suggested plugin not installed, no instructions, no reviewers, a
/// repository config Keel had to quarantine. Each says what to do about it.
export interface Warning {
  id: string;
  severity: "critical" | "high" | "medium";
  title: string;
  detail: string;
  /// Where the fix is: Settings (log in, install), Extensions (plugins), Readiness (the rest).
  fix: "settings" | "extensions" | "readiness";
  /// For a suggested plugin: where it installs from.
  marketplace?: string;
  /// For a scanner finding: the scanner's own recipe, which is what the check looks for. The
  /// detail says what is wrong; an agent handed only that may fix it without satisfying the check.
  recipe?: string;
  path?: string;
}

/// Whether this one actually stops a lane from working: Claude Code missing or logged out. A
/// repository's hook script is critical to *know* about, but Keel sets it aside before every
/// run, so it blocks nothing — and calling it "Blocks lanes" said something untrue.
export const blocks = (w: Warning) => w.id.startsWith("claude/");

/// Where a warning lives, which is where its fix lands: the machine (Claude Code itself) or this
/// folder (its files, its plugins). The popover's third group, this session, is not a property of
/// the warning but of what was handed to a lane — see `handed` in the store.
export const scope = (w: Warning): "global" | "folder" => (blocks(w) || w.id === "keel/unread-claude" || w.id.startsWith("license/") ? "global" : "folder");

interface Finding {
  id: string;
  severity: "critical" | "high" | "medium" | "low";
  title: string;
  detail: string;
  path?: string | null;
  fix?: { description: string };
}
export interface Meter {
  steps: number;
  cap: number | null;
  licensee: string | null;
  email: string;
  /// The key's last valid day (UTC), or null on the free tier.
  expires: string | null;
}
interface Plugin {
  name: string;
  marketplace: string;
  installed: boolean;
  reason?: string | null;
}

/// The scanner's findings about agent configuration, as opposed to production readiness (CI,
/// Dockerfiles), which is the Readiness tab's business and not a warning on every glance.
const CONFIG = /^(agent\/|security\/untrusted-agent-config$|docs\/no-readme$)/;

/// Read every source once, in parallel. A source that fails says so as a warning of its own
/// rather than vanishing: a list that is silently short reads as "all fine".
/// How many findings the Readiness tab has open, and whether any is critical: the tab's badge.
export interface Readiness {
  open: number;
  critical: boolean;
}

export async function readSetup(ep: Endpoint, path: string): Promise<{ warnings: Warning[]; readiness?: Readiness; license?: Meter }> {
  const [claude, state, plugins, meter, team] = await Promise.allSettled([
    get<{ installed: boolean; authenticated: boolean }>(ep, "/api/claude"),
    get<{ scan: { findings: Finding[]; ignored: string[] } }>(ep, "/api/state"),
    get<{ suggested: Plugin[] }>(ep, "/api/plugins", { repo: path }),
    get<Meter | null>(ep, "/api/license"),
    readTeam(ep),
  ]);
  const out: Warning[] = [];
  if (claude.status === "fulfilled") {
    if (!claude.value.installed) out.push({ id: "claude/not-installed", severity: "critical", title: "Claude Code is not installed", detail: "Lanes run the `claude` CLI, and it is not on this machine's PATH.", fix: "settings" });
    else if (!claude.value.authenticated) out.push({ id: "claude/not-logged-in", severity: "critical", title: "Claude Code is not logged in", detail: "Every turn will fail until it is.", fix: "settings" });
  } else out.push(unread("claude", "whether Claude Code is installed", claude.reason));
  let readiness: Readiness | undefined;
  if (state.status === "fulfilled") {
    const ignored = new Set(state.value.scan.ignored);
    const open = state.value.scan.findings.filter((f) => !ignored.has(f.id));
    readiness = { open: open.length, critical: open.some((f) => f.severity === "critical") };
    for (const f of state.value.scan.findings)
      if (CONFIG.test(f.id) && f.severity !== "low" && !ignored.has(f.id)) out.push({ id: f.id, severity: f.severity as Warning["severity"], title: f.title, detail: f.detail, path: f.path ?? undefined, fix: "readiness", recipe: f.fix?.description });
  } else out.push(unread("scan", "the project's configuration", state.reason));
  if (plugins.status === "fulfilled") {
    for (const p of plugins.value.suggested)
      if (!p.installed) out.push({ id: `plugin/${p.name}`, severity: "medium", title: `Plugin “${p.name}” is not installed`, detail: p.reason ?? "Suggested for this project.", fix: "extensions", marketplace: p.marketplace });
  } else out.push(unread("plugins", "the plugin catalog", plugins.reason));
  // Newer team files for a project that has the team: an offer, not a problem, so it never moves
  // the readiness score and a team nobody added stays the scanner's finding alone.
  if (team.status === "fulfilled" && offered(team.value))
    out.push({ id: "team/update", severity: "medium", title: "Team update available", detail: `${pending(team.value)} files from Keel's agent team are out of date.`, fix: "extensions" });
  // The free tier's daily steps: said before they run out, so a refused step is never the first
  // anyone hears of the limit. A meter that cannot be read says nothing — it is not setup.
  if (meter.status === "fulfilled" && meter.value?.cap != null) {
    const { steps, cap, email } = meter.value;
    if (steps >= cap)
      out.push({ id: "license/daily-limit", severity: "critical", title: `Today's ${cap} free agent steps are used`, detail: `Agents can read but not run commands or edit until 00:00 UTC. Teams, companies and more steps: ${email}.`, fix: "settings" });
    else if (steps >= cap * 0.8)
      out.push({ id: "license/daily-limit", severity: "medium", title: `${steps} of ${cap} free agent steps used today`, detail: `The count resets at 00:00 UTC. Teams, companies and more steps: ${email}.`, fix: "settings" });
  }
  const rank = { critical: 0, high: 1, medium: 2 };
  return { warnings: out.sort((a, b) => rank[a.severity] - rank[b.severity]), readiness, license: meter.status === "fulfilled" ? (meter.value ?? undefined) : undefined };
}

const unread = (id: string, what: string, why: unknown): Warning => ({
  id: `keel/unread-${id}`,
  severity: "medium",
  title: `Could not check ${what}`,
  detail: why instanceof Error ? why.message : String(why),
  fix: "settings",
});
