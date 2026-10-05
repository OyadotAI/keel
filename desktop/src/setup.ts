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
}

interface Finding {
  id: string;
  severity: "critical" | "high" | "medium" | "low";
  title: string;
  detail: string;
}
interface Plugin {
  name: string;
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
  const [claude, state, plugins] = await Promise.allSettled([
    get<{ installed: boolean; authenticated: boolean }>(ep, "/api/claude"),
    get<{ scan: { findings: Finding[]; ignored: string[] } }>(ep, "/api/state"),
    get<{ suggested: Plugin[] }>(ep, "/api/plugins", { repo: path }),
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
      if (CONFIG.test(f.id) && f.severity !== "low" && !ignored.has(f.id)) out.push({ id: f.id, severity: f.severity as Warning["severity"], title: f.title, detail: f.detail, fix: "readiness" });
  } else out.push(unread("scan", "the project's configuration", state.reason));
  if (plugins.status === "fulfilled") {
    for (const p of plugins.value.suggested)
      if (!p.installed) out.push({ id: `plugin/${p.name}`, severity: "medium", title: `Plugin “${p.name}” is not installed`, detail: p.reason ?? "Suggested for this project.", fix: "extensions" });
  } else out.push(unread("plugins", "the plugin catalog", plugins.reason));
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
