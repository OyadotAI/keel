// The workspace and Settings share the same explicit project-trust disclosure and decision.
import { projectTrust, useStore } from "../store";
import { useEffect, useRef } from "react";
import type { TrustRequest } from "../project-trust";
import { Banner, Dialog } from "./kit";
import "./settings/Settings.css";

/** Keep the provider limitation visible without depending on an optional capability read. */
export function TrustLimitations() {
  return <p className="trust-limitations">In Antigravity lanes, deny rules still apply to commands, MCP tools and web fetches. Read, Edit and Write path deny rules do not apply.</p>;
}

/** Mount once above the workspace so switching lanes cannot retarget or dismiss consent. */
export function TrustDialog() {
  const request = useStore((s) => s.trustRequest);
  const name = useStore((s) => request ? s.projects[request.project]?.name : undefined);
  return request && <TrustDialogView request={request} name={name} confirm={() => void projectTrust.confirm()} retry={projectTrust.retry} close={projectTrust.dismiss} />;
}

/** Present every connection/save state with cancellation first and no implicit permission write. */
export function TrustDialogView({ request, name, confirm, retry, close }: {
  request: TrustRequest; name?: string; confirm: () => void; retry: () => void; close: () => void;
}) {
  const busy = request.phase === "saving";
  const pending = useRef<HTMLParagraphElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (busy) pending.current?.focus();
    else if (request.error) cancel.current?.focus();
  }, [busy, request.error]);
  const dismiss = () => { if (!busy) close(); };
  return <Dialog title="Trust this project?" onClose={dismiss}>
    <div className="trust-dialog-content" aria-busy={busy}>
      <div className="trust-project-identity"><strong>{name ?? request.project.split(/[\\/]/).filter(Boolean).pop()}</strong><code>{request.project}</code></div>
      <ul className="trust-disclosure">
        <li>Agents can make permitted file edits, run commands and use MCP tools without asking—including installs, deletes, network calls and Keel Browser actions.</li>
        <li>Keel can install and update its agent team, skills and suggested plugins from Anthropic’s official marketplace here, as its own commits.</li>
        <li>Explicit deny rules still apply. Questions and plan decisions still need your answer.</li>
      </ul>
      <p>Trust applies only to this project and lasts until you choose <strong>Stop trusting</strong> in Settings → Project trust.</p>
      <TrustLimitations />
      {request.phase === "connecting" && <p role="status">Connecting…</p>}
      {busy && <p ref={pending} tabIndex={0} role="status">Saving project trust…</p>}
      {request.error && <Banner tone="error">{request.error}</Banner>}
    </div>
    <div className="actions">
      <button ref={cancel} disabled={busy} onClick={dismiss}>Keep asking</button>
      {request.phase === "failed" ? <button onClick={retry}>Try again</button> : <button className="primary" disabled={request.phase !== "ready"} onClick={confirm}>{busy ? "Trusting…" : "Trust this project"}</button>}
    </div>
  </Dialog>;
}
