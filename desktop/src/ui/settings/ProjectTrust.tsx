// Settings makes project trust prominent while retaining the daemon's existing permission rules.
import { useEffect, useRef, useState } from "react";
import { get, post, type Endpoint } from "../../api";
import { projectTrust, useStore } from "../../store";
import { Banner } from "../kit";
import { Icon } from "../icons";
import { TrustLimitations } from "../TrustDialog";

interface Permissions { project: string[]; session: string[]; suggested: string[]; trusted: boolean }
interface Props { project?: string; name?: string; endpoint?: Endpoint; error?: string; retry: () => void }

/** Read authoritative permissions again after a confirmed grant or a local rule mutation. */
function usePermissions(project?: string, endpoint?: Endpoint) {
  const confirmed = useStore((s) => project ? s.trusted[project] : undefined);
  const [permissions, setPermissions] = useState<Permissions>();
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let live = true;
    if (endpoint) void get<Permissions>(endpoint, "/api/permissions").then(
      (value) => { if (live) { setPermissions(value); setError(undefined); } },
      (reason) => { if (live) setError(String(reason)); },
    );
    return () => { live = false; };
  }, [endpoint, confirmed, revision]);
  return { permissions, error, refresh: () => setRevision((n) => n + 1) };
}

/** Pin mutations to this mounted project and keep its last confirmed state on failure. */
function usePermissionChanges(project: string | undefined, endpoint: Endpoint | undefined, refresh: () => void) {
  const pending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  async function change(path: string, body: object) {
    if (!endpoint || !project || pending.current) return;
    pending.current = true;
    setBusy(true);
    try {
      const result = await post(endpoint, path, body);
      if (path === "/api/permissions/trust") {
        if (result !== false) throw new Error("Keel did not confirm that trust was withdrawn. Try again.");
        useStore.setState((s) => s.projects[project] ? { trusted: { ...s.trusted, [project]: false } } : {});
      }
      setError(undefined);
      refresh();
    } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
    finally { pending.current = false; setBusy(false); }
  }
  return { change, busy, error };
}

/** Show the project identity before loading so trust is always the first discoverable setting. */
export function ProjectTrust({ project, name, endpoint, error: connectionError, retry }: Props) {
  const { permissions, error: readError, refresh } = usePermissions(project, endpoint);
  const { change, busy, error: mutationError } = usePermissionChanges(project, endpoint, refresh);
  const error = connectionError ?? readError;
  return <>
    <section className="project-trust-surface" aria-labelledby="settings-trust" aria-busy={busy}>
      <div className="settings-eyebrow"><Icon name="shield" size={15} /> This project</div>
      <h2 id="settings-trust" tabIndex={-1}>Project trust</h2>
      {project && <div className="trust-project-identity"><strong>{name ?? project}</strong><code>{project}</code></div>}
      {!project ? <p>Open a project to manage trust and permission rules.</p> : error ? <>
        <Banner tone="error">Couldn’t load project trust. {error}</Banner>
        <button onClick={connectionError ? retry : refresh}>Try again</button>
      </> : !permissions ? <p role="status">Connecting to this project…</p> : <>
        <div className="trust-summary">
          <div><strong className="trust-state"><span className={`trust-state-dot ${permissions.trusted ? "is-trusted" : ""}`} />{permissions.trusted ? "Trusted" : "Asks for permission"}</strong>
            <p>{permissions.trusted ? "Permitted actions run without asking. Explicit deny rules still apply." : "Agents ask before actions outside your allowed rules."}</p></div>
          <button className={permissions.trusted ? undefined : "primary"} disabled={busy} onClick={() => permissions.trusted ? void change("/api/permissions/trust", { trusted: false }) : projectTrust.open(project)}>{busy ? "Saving…" : permissions.trusted ? "Stop trusting" : "Trust this project…"}</button>
        </div>
        <p className="trust-automation">Keel also keeps its agent team, skills and suggested official-marketplace plugins current here as its own commits. Questions and plan decisions still need your answer.</p>
        <TrustLimitations />
        {mutationError && <Banner tone="error">{mutationError}</Banner>}
      </>}
    </section>
    {permissions && endpoint && !error && <PermissionRules permissions={permissions} busy={busy} change={change} />}
  </>;
}

/** Rules remain outside the trust summary so a long allowlist cannot bury the grant action. */
function PermissionRules({ permissions, busy, change }: { permissions: Permissions; busy: boolean; change: (path: string, body: object) => Promise<void> }) {
  const suggested = permissions.suggested.filter((rule) => !permissions.project.includes(rule));
  return <div className="settings-permissions">
    <h3>Allowed without asking</h3>
    {permissions.project.length ? <ul className="rules">{permissions.project.map((rule) => <li key={rule}><code>{rule}</code><button className="link" disabled={busy} aria-label={`Remove ${rule}`} onClick={() => void change("/api/permissions/remove", { rule, scope: "project" })}>Remove</button></li>)}</ul> : <p className="muted">No rules yet. Each “Allow” you give for this project can become one.</p>}
    {suggested.length > 0 && <><h3>Suggested for this project</h3><ul className="rules">{suggested.map((rule) => <li key={rule}><code>{rule}</code><button className="link" disabled={busy} onClick={() => void change("/api/permissions/add", { rule, scope: "project" })}>Allow</button></li>)}</ul></>}
  </div>;
}
