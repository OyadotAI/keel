import { InterfaceSetting } from "./InterfaceSetting";
import { check, useUpdate } from "../update";
import { useCallback, useEffect, useRef, useState } from "react";
import { get, post, url, type Endpoint } from "../api";
import { open as openStream } from "../streams";
import { useStore } from "../store";

interface Claude {
  installed: boolean;
  version?: string | null;
  authenticated: boolean;
  account?: string | null;
  plan?: string | null;
}

interface Permissions {
  project: string[];
  session: string[];
  suggested: string[];
  trusted: boolean;
}

/// Settings for the project the active lane is in, and for Claude Code itself.
export function Settings() {
  const project = useStore((s) => (s.active ? s.lanes[s.active]?.project : undefined) ?? s.order[0]);
  const name = useStore((s) => (project ? s.projects[project]?.name : undefined));
  const [ep, setEp] = useState<Endpoint>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    if (!project) return;
    useStore.getState().ensure(project).then(setEp, (e) => setError(String(e)));
  }, [project]);
  const close = () => useStore.setState({ settings: false });
  return (
    <div className="settings">
      <header className="lane-header">
        <div className="lane-heading">
          <span className="lane-name">Settings</span>
          <span className="muted small">{name ? `for ${name} and for Claude Code` : "Open a project to change its settings"}</span>
        </div>
        <button onClick={close}>Done</button>
      </header>
      <div className="settings-body">
        <InterfaceSetting applyToLane />
        {error && <div className="error">{error}</div>}
        {!project ? (
          <p className="muted">Settings belong to a project. Open one first.</p>
        ) : !ep ? (
          <p className="muted">Starting Keel for {name}…</p>
        ) : (
          <>
            <TrustSection ep={ep} project={project} />
            <ClaudeSection ep={ep} />
          </>
        )}
        <UpdatesSection />
      </div>
    </div>
  );
}

function ClaudeSection({ ep }: { ep: Endpoint }) {
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
    );
  }
  return (
    <section className="settings-section">
      <h2>Claude Code</h2>
      {!claude ? (
        <p className="muted">Checking…</p>
      ) : !claude.installed ? (
        <>
          <p>Claude Code is not installed. Keel runs your own <code>claude</code>, so it needs it first.</p>
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

function TrustSection({ ep, project }: { ep: Endpoint; project: string }) {
  const [p, setP] = useState<Permissions>();
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string>();
  const read = useCallback(() => get<Permissions>(ep, "/api/permissions").then(setP, (e) => setError(String(e))), [ep]);
  useEffect(() => void read(), [read]);
  async function change(fn: () => Promise<unknown>) {
    try {
      await fn();
      setError(undefined);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    // The status-bar marker follows the daemon's `permissions.changed`, not this page.
    await read();
  }
  if (!p) return <section className="settings-section"><h2>This project</h2><p className="muted">Reading…</p></section>;
  return (
    <section className="settings-section">
      <h2>This project</h2>
      {error && <div className="error">{error}</div>}
      {p.trusted ? (
        <p>
          <strong>Trusted.</strong> The agent runs commands here without asking. Rules you approved are kept if you withdraw it.{" "}
          <button onClick={() => change(() => post(ep, "/api/permissions/trust", { trusted: false }))}>Stop trusting</button>
        </p>
      ) : asking ? (
        <div className="card">
          <p>
            Trusting <code>{project}</code> lets the agent run <strong>any command in this project without asking you</strong> — installs,
            deletes, network calls — for as long as it is trusted. It applies to this project only, is shown while it holds, and you can withdraw it here.
          </p>
          <div className="card-actions">
            <button className="primary" onClick={() => change(() => post(ep, "/api/permissions/trust", { trusted: true })).then(() => setAsking(false))}>
              Trust this project
            </button>
            <button onClick={() => setAsking(false)}>Keep asking</button>
          </div>
        </div>
      ) : (
        <p>
          The agent asks before each command outside the rules below.{" "}
          <button onClick={() => setAsking(true)}>Trust this project…</button>
        </p>
      )}
      <h3>Allowed without asking</h3>
      {p.project.length === 0 ? (
        <p className="muted small">No rules yet. Each "Allow" you give for this project can become one.</p>
      ) : (
        <ul className="rules">
          {p.project.map((r) => (
            <li key={r}>
              <code>{r}</code>
              <button className="ghost" aria-label={`Remove ${r}`} onClick={() => change(() => post(ep, "/api/permissions/remove", { rule: r, scope: "project" }))}>
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}
      {p.suggested.filter((r) => !p.project.includes(r)).length > 0 && (
        <>
          <h3>Suggested for this project</h3>
          <ul className="rules">
            {p.suggested
              .filter((r) => !p.project.includes(r))
              .map((r) => (
                <li key={r}>
                  <code>{r}</code>
                  <button className="ghost" onClick={() => change(() => post(ep, "/api/permissions/add", { rule: r, scope: "project" }))}>
                    Allow
                  </button>
                </li>
              ))}
          </ul>
        </>
      )}
    </section>
  );
}

/// Which version this is, and whether a newer one exists — asked on demand as well as on the
/// six-hourly check.
function UpdatesSection() {
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
            ? `Keel ${u.version} is downloaded — restart from the status bar to install it.`
            : u.state === "failed"
              ? `Could not check: ${u.why}`
              : import.meta.env.DEV
                ? "A development build does not update itself."
                : "";
  return (
    <section className="settings-section">
      <h2>Updates</h2>
      <p className="small muted">
        Keel {version ?? "…"} · checks for a new version at launch and every six hours, downloads it in the background, and installs it when you restart.
      </p>
      <div className="form-row">
        <button onClick={() => void check()} disabled={u.state === "checking" || u.state === "downloading"}>
          Check for updates
        </button>
        {said && <span className="small faint">{said}</span>}
      </div>
    </section>
  );
}
