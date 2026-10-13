import { get, type Endpoint } from "./api";

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
  fix?: { description: string };
}
interface Meter {
  steps: number;
  cap: number | null;
  licensee: string | null;
  email: string;
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

export async function readSetup(ep: Endpoint, path: string): Promise<{ warnings: Warning[]; readiness?: Readiness }> {
  const [claude, state, plugins, meter] = await Promise.allSettled([
    get<{ installed: boolean; authenticated: boolean }>(ep, "/api/claude"),
    get<{ scan: { findings: Finding[]; ignored: string[] } }>(ep, "/api/state"),
    get<{ suggested: Plugin[] }>(ep, "/api/plugins", { repo: path }),
    get<Meter | null>(ep, "/api/license"),
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
      if (CONFIG.test(f.id) && f.severity !== "low" && !ignored.has(f.id)) out.push({ id: f.id, severity: f.severity as Warning["severity"], title: f.title, detail: f.detail, fix: "readiness", recipe: f.fix?.description });
  } else out.push(unread("scan", "the project's configuration", state.reason));
  if (plugins.status === "fulfilled") {
    for (const p of plugins.value.suggested)
      if (!p.installed) out.push({ id: `plugin/${p.name}`, severity: "medium", title: `Plugin “${p.name}” is not installed`, detail: p.reason ?? "Suggested for this project.", fix: "extensions", marketplace: p.marketplace });
  } else out.push(unread("plugins", "the plugin catalog", plugins.reason));
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
  return { warnings: out.sort((a, b) => rank[a.severity] - rank[b.severity]), readiness };
}

const unread = (id: string, what: string, why: unknown): Warning => ({
  id: `keel/unread-${id}`,
  severity: "medium",
  title: `Could not check ${what}`,
  detail: why instanceof Error ? why.message : String(why),
  fix: "settings",
});
