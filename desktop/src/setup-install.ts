import { url } from "./api";
import { open as openStream } from "./streams";

/// A plugin install, to its end. The CLI streams; this waits for it to finish either way.
export function install(ep: { port: number; token: string }, name: string, marketplace: string): Promise<string | undefined> {
  return new Promise((done) => {
    let ok = false;
    const output: string[] = [];
    openStream(
      url(ep, "/api/plugins/install", { name, marketplace, commit: true }),
      ep.token,
      (events) => {
        for (const e of events) {
          if (e.event === "done") ok = String(e.data) === "0";
          if (e.event === "line" || e.event === "fatal") output.push(String(e.data));
        }
      },
      (error) => done(error || (ok ? undefined : output.slice(-8).join("\n") || "Installer ended without confirming success.")),
    );
  });
}
