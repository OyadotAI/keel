// Existing connection and update controls stay mounted while Settings navigation moves.
import { useCallback, useEffect, useRef, useState } from "react";
import { get, url, type Endpoint } from "../api";
import { open as openStream } from "../streams";
import { check, useUpdate } from "../update";

interface Claude {
  installed: boolean;
  version?: string | null;
  authenticated: boolean;
  account?: string | null;
  plan?: string | null;
}

/** Retain installation and sign-in streams while the person moves between settings sections. */
export function ClaudeSection({ ep }: { ep: Endpoint }) {
  const [claude, setClaude] = useState<Claude>();
  const [log, setLog] = useState<string[]>([]);
  const [busy, setBusy] = useState<string>();
  // The running install or login. Closed by Cancel and by leaving the page: a login waits on the
  // browser for as long as it takes, and closing the stream is what ends the process behind it.
  const stream = useRef<{ close(): void }>(undefined);
  const read = useCallback(() => get<Claude>(ep, "/api/claude").then(setClaude, () => undefined), [ep]);
  useEffect(() => {
    void read();
    return () => stream.current?.close();
  }, [read]);
  function cancel() {
    stream.current?.close();
    stream.current = undefined;
    setBusy(undefined);
    void read();
  }
  function run(action: "install" | "login") {
    setBusy(action);
    setLog([]);
    stream.current = openStream(
      url(ep, `/api/claude/${action}`),
      ep.token,
      (events) => {
        const lines = events.filter((e) => e.event === "line" || e.event === "fatal").map((e) => String(e.data));
        if (lines.length) setLog((l) => [...l, ...lines].slice(-200));
      },
      () => {
        stream.current = undefined;
        setBusy(undefined);
        void read();
      },
      // The install runs a downloaded script, so its route takes POST only.
      action === "install" ? "POST" : "GET",
    );
  }
  return (
    <section className="settings-section">
      {!claude ? (
        <p className="muted">Checking…</p>
      ) : !claude.installed ? (
        <>
          <p>Claude Code is not installed. Oya Factory runs your own <code>claude</code>, so it needs it first.</p>
          <button className="primary" disabled={!!busy} onClick={() => run("install")}>
            {busy === "install" ? "Installing…" : "Install Claude Code"}
          </button>
        </>
      ) : !claude.authenticated ? (
        <>
          <p>
            Installed ({claude.version}), but not logged in. Logging in opens your browser; this page shows what it says.
          </p>
          <button className="primary" disabled={!!busy} onClick={() => run("login")}>
            {busy === "login" ? "Waiting for the browser…" : "Log in"}
          </button>
        </>
      ) : (
        <p>
          {claude.version} · logged in{claude.account ? ` as ${claude.account}` : ""}
          {claude.plan ? ` · ${claude.plan}` : ""}
        </p>
      )}
      {busy && (
        <button className="link small" onClick={cancel}>
          Cancel
        </button>
      )}
      {log.length > 0 && <Log lines={log} />}
    </section>
  );
}

/// Output with its links clickable: a login prints the URL to finish it at.
function Log({ lines }: { lines: string[] }) {
  return (
    <pre className="code">
      {lines.map((l, i) => (
        <div key={i}>
          {l.split(/(https?:\/\/\S+)/).map((part, j) =>
            /^https?:\/\//.test(part) ? (
              <a key={j} href={part}>
                {part}
              </a>
            ) : (
              part
            ),
          )}
        </div>
      ))}
    </pre>
  );
}

/// Which version this is, and whether a newer one exists — asked on demand as well as on the
/// six-hourly check.
export function UpdatesSection() {
  const u = useUpdate();
  const [version, setVersion] = useState<string>();
  useEffect(() => {
    void import("@tauri-apps/api/app").then((a) => a.getVersion()).then(setVersion, () => undefined);
  }, []);
  const said =
    u.state === "checking"
      ? "Checking…"
      : u.state === "current"
        ? "This is the latest version."
        : u.state === "downloading"
          ? `Downloading ${u.version}…`
          : u.state === "ready"
            ? `Oya Factory ${u.version} is downloaded — choose Update now in the header to install it.`
            : u.state === "installing"
              ? `Installing Oya Factory ${u.version}…`
            : u.state === "failed"
              ? `Could not check: ${u.why}`
              : import.meta.env.DEV
                ? "A development build does not update itself."
                : "";
  return (
    <section className="settings-section">
      <h2>Updates</h2>
      <p className="small muted">
        Oya Factory {version ?? "…"} · checks for a new version at launch and every six hours, downloads it in the background, and installs it when you restart.
      </p>
      <div className="form-row">
        <button onClick={() => void check()} disabled={u.state === "checking" || u.state === "downloading" || u.state === "ready" || u.state === "installing"}>
          Check for updates
        </button>
        {said && <span className="small faint">{said}</span>}
      </div>
    </section>
  );
}
