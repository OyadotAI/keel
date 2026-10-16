import { useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { url } from "../api";
import { start, useRun } from "../runs";
import { useStore } from "../store";
import { reviewEvidence, recordedCheck, reviewScope, reviewVerdict, checkStep, checkEnd, commandEvidence, commandMarkdown, verificationSummary, type ReviewCheck } from "../review";
import { Icon } from "./icons";

/// Readiness and exports share a verdict tied to the latest conversation and checkout events.
export function Review({ lane, ask }: { lane: string; ask: (what: "merge" | "discard") => void }) {
  const l = useStore((s) => s.lanes[lane]);
  const ep = useStore((s) => (l ? s.projects[l.project]?.endpoint : undefined));
  const projectName = useStore((s) => (l ? s.projects[l.project]?.name : ""));
  const runKey = `verify:${lane}`;
  const run = useRun<ReviewCheck>(runKey)?.state;
  const [note, setNote] = useState<string>();
  // Derived once per change to the turns, not once per render: the lane object changes on every
  // batch of a streaming turn, and this walks every call in the conversation.
  const turns = l?.conv.turns;
  const { worked, files, commands, failed, latest } = useMemo(() => reviewEvidence(turns), [turns]);
  const recorded = recordedCheck(latest);
  const scope = reviewScope(l?.session, l?.wt, l?.reviewRevision ?? 0, recorded, ep?.port);
  const verdict = reviewVerdict(!!l?.running, run, scope, recorded, latest?.gate);
  const blocker = verdict.blocker;
  const runCurrent = !!run && (run.scope === scope || run.state === "running");
  if (!l) return null;

  function check() {
    if (!ep || l?.running || run?.state === "running") return;
    start<ReviewCheck>(
      runKey,
      url(ep, "/api/verify", { wt: l!.wt }),
      ep.token,
      { state: "running", problems: [], scope, recorded, started_at: new Date().toISOString() },
      checkStep,
      checkEnd,
    );
  }

  function payload() {
    return {
      schema: 2,
      taskID: l!.id,
      title: l!.title,
      provider: l!.agent,
      repository: projectName,
      branch: l!.git?.branch ?? (l!.wt ? `keel/${l!.wt}` : null),
      worktree: l!.wt ?? null,
      head: [...worked].reverse().find((t) => t.commit)?.commit ?? null,
      files,
      commands: commands.map(commandEvidence),
      gates: worked.map((t) => ({ status: t.gate?.status ?? "none", command: t.gate?.command ?? null })),
      verification: { status: verdict.state, source: verdict.source, command: verdict.command ?? null, ...(runCurrent && run ? { started_at: run.started_at, finished_at: run.finished_at ?? null, problems: run.problems, why: run.why ?? null } : {}) },
      ready: !blocker,
      blocker: blocker ?? null,
    };
  }
  function markdown() {
    const p = payload();
    return [
      `# ${p.title}`,
      ``,
      `- Repository: ${p.repository}`,
      `- Branch: ${p.branch ?? "the project's working tree"}${p.head ? ` at ${p.head.slice(0, 7)}` : ""}`,
      `- Agent: ${p.provider}`,
      `- Checks: ${verdict.state} (${verdict.source})${blocker ? ` — ${blocker}` : ""}`,
      ``,
      `## Files changed (${files.length})`,
      ...files.map((f) => `- \`${f}\``),
      ``,
      `## Commands (${commands.length}${failed ? `, ${failed} failed` : ""})`,
      ...p.commands.slice(0, 100).map(commandMarkdown),
      ``,
    ].join("\n");
  }
  async function exportSigned() {
    setNote(undefined);
    try {
      // Canonical: the exact bytes signed are the bytes saved.
      const body = JSON.stringify(payload());
      const signed = await invoke<{ signature: string; public_key: string }>("sign_packet", { payload: body });
      const packet = JSON.stringify({ payload: JSON.parse(body), signed: body, signature: signed.signature, publicKey: signed.public_key, algorithm: "ed25519" }, null, 2);
      const saved = await invoke<boolean>("save_text", { name: `${l!.title.replace(/[^\w.-]+/g, "-").slice(0, 60) || "review"}.keel-review.json`, contents: packet });
      if (saved) setNote("Saved a signed review packet. It holds the evidence — files, commands, checks — never source, diffs, prompts or command output.");
    } catch (e) {
      setNote(`Could not export: ${e}`);
    }
  }

  return (
    <div style={{ overflowY: "auto", height: "100%", padding: "12px 16px" }}>
      <div className="card-head">
        <span className="verdict" style={{ color: blocker ? "var(--warn)" : "var(--add)" }}>
          <Icon name={blocker ? "hand" : "check-circle"} size={14} />
          {blocker ? "Not ready" : "Ready to merge"}
        </span>
      </div>
      {blocker && <p className="small muted" style={{ margin: "4px 0 0" }}>{blocker}</p>}

      <h3 className="eyebrow" style={{ margin: "16px 0 6px" }}>Where it goes</h3>
      {l.wt ? (
        <p className="mono small" style={{ margin: 0 }}>
          keel/{l.wt} → the branch it was cut from
        </p>
      ) : (
        <p className="small muted" style={{ margin: 0 }}>Shares the project's working tree — no branch of its own to merge. Its turns are commits on {l.git?.branch ?? "the current branch"}.</p>
      )}
      <div className="actions">
        {l.wt && (
          <>
            <button className="primary" disabled={!!blocker || !ep} onClick={() => ask("merge")}>
              Merge…
            </button>
            <button onClick={() => ask("discard")}>Discard…</button>
          </>
        )}
      </div>

      <h3 className="eyebrow" style={{ margin: "18px 0 6px" }}>The project's checks</h3>
      <div className="actions" style={{ marginTop: 0 }}>
        <button onClick={check} disabled={l.running || run?.state === "running" || !ep}>
          {run?.state === "running" ? "Running…" : "Run the checks now"}
        </button>
        {run?.command && <code className="small">{run.command}</code>}
      </div>
      <p className="small" style={{ color: blocker ? "var(--dim)" : "var(--add)" }}>
        {verificationSummary(verdict, runCurrent ? run : undefined)}
      </p>
      {run && !runCurrent && <p className="small muted">The earlier manual result belongs to a previous checkout or conversation state.</p>}
      {runCurrent && run?.problems.slice(0, 30).map((p, i) => (
        <div key={i} className="problem"><span className="del">✗</span><span>{p.message}</span><span className="where">{p.file}:{p.line}</span></div>
      ))}
      {runCurrent && run?.why && <p className="small muted">{run.why}</p>}
      {!runCurrent && <p className="small muted">{worked.length} turn{worked.length === 1 ? "" : "s"} did work; latest turn checks: <strong>{latest?.gate?.status ?? "not run"}</strong>.</p>}

      <h3 className="eyebrow" style={{ margin: "18px 0 6px" }}>
        {files.length} file{files.length === 1 ? "" : "s"} · {commands.length} command{commands.length === 1 ? "" : "s"}
        {failed ? ` · ${failed} failed` : ""}
      </h3>
      {files.slice(0, 40).map((f) => (
        <div key={f} className="mono small" style={{ padding: "1px 0" }}>
          {f}
        </div>
      ))}

      <h3 className="eyebrow" style={{ margin: "18px 0 6px" }}>Review packet</h3>
      <p className="small muted" style={{ marginTop: 0 }}>
        A summary to hand a reviewer: the files, the commands and the checks — never source code, diffs, prompts or command output.
      </p>
      <div className="actions" style={{ marginTop: 0 }}>
        <button onClick={() => void navigator.clipboard.writeText(markdown()).then(() => setNote("Copied the summary as Markdown."), (error) => setNote(`Could not copy: ${error}`))}>
          <Icon name="copy" size={14} /> Copy as Markdown
        </button>
        <button onClick={exportSigned}>Export signed packet…</button>
      </div>
      {note && <p className="small faint">{note}</p>}
    </div>
  );
}
