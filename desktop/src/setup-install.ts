import { post, url, type Endpoint } from "./api";
import type { Warning } from "./setup";
import { open as openStream } from "./streams";

/// A plugin install, to its end. The CLI streams; this waits for it to finish either way.
export function install(ep: Endpoint, name: string, marketplace: string, onLine?: (line: string) => void): Promise<string | undefined> {
  return new Promise((done) => {
    let ok = false;
    const output: string[] = [];
    openStream(
      url(ep, "/api/plugins/install", { name, marketplace, commit: true }),
      ep.token,
      (events) => {
        for (const e of events) {
          if (e.event === "done") ok = String(e.data) === "0";
          if (e.event === "line" || e.event === "fatal") {
            const line = String(e.data);
            output.push(line);
            if (output.length > 100) output.shift();
            onLine?.(line);
          }
        }
      },
      (error) => done(error || (ok ? undefined : output.slice(-8).join("\n") || "Installer ended without confirming success.")),
    );
  });
}

interface TeamResult {
  written: string[];
  skipped: string[];
  note?: string | null;
}

/** Install the local team before network downloads, reporting each outcome independently. */
export async function installSetup(
  ep: Endpoint,
  warnings: Warning[],
  progress: (message: string) => void,
  teamChanged: () => void | Promise<void>,
): Promise<{ installed: number; total: number; team?: TeamResult; errors: string[] }> {
  const errors: string[] = [];
  let team: TeamResult | undefined;
  if (warnings.some((w) => w.id.startsWith("agent/"))) {
    progress("Adding the agent team…");
    try {
      team = await post<TeamResult>(ep, "/api/adopt", undefined, { commit: true });
      await teamChanged();
      for (const skipped of team.skipped) {
        if (!/\(already (?:there|says it)\)$/.test(skipped)) errors.push(`Agent team: ${skipped}`);
      }
    } catch (error) {
      errors.push(`Agent team: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  const plugins = warnings.filter((w) => w.id.startsWith("plugin/") && w.marketplace);
  for (const w of warnings.filter((w) => w.id.startsWith("plugin/") && !w.marketplace))
    errors.push(`${w.id.slice(7)}: no marketplace is known for it — install it from Extensions.`);
  let installed = 0;
  for (const [i, w] of plugins.entries()) {
    const name = w.id.slice(7);
    const status = `Installing ${name} (${i + 1} of ${plugins.length})…`;
    progress(status);
    try {
      const error = await install(ep, name, w.marketplace!, (line) => progress(`${status}\n${line}`));
      if (error) errors.push(`${name}: ${error}`);
      else installed++;
    } catch (error) {
      errors.push(`${name}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  return { installed, total: plugins.length, team, errors };
}
