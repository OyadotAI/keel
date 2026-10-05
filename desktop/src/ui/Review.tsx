import { useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { url } from "../api";
import { start, useRun } from "../runs";
import { useStore } from "../store";
import type { Turn } from "../reduce";
import { Icon } from "./icons";

interface Problem {
  file: string;
  line: number;
  severity: string;
  message: string;
}

/// The merge decision, on one screen: where the work goes, what it touched, what ran, and whether
/// the project's own checks pass on the tree as it stands — run here, not remembered.
type Check = { state: "running" | "passed" | "failed" | "none"; command?: string; problems: Problem[]; why?: string };

export function Review({ lane, ask }: { lane: string; ask: (what: "merge" | "discard") => void }) {
  const l = useStore((s) => s.lanes[lane]);
  const ep = useStore((s) => (l ? s.projects[l.project]?.endpoint : undefined));
  const projectName = useStore((s) => (l ? s.projects[l.project]?.name : ""));
  const runKey = `verify:${lane}`;
  const run = useRun<Check>(runKey)?.state;
  const [note, setNote] = useState<string>();
  // Derived once per change to the turns, not once per render: the lane object changes on every
  // batch of a streaming turn, and this walks every call in the conversation.
  const turns = l?.conv.turns;
  const { worked, files, commands, failed, last } = useMemo(() => {
    const worked = (turns ?? []).filter((t) => t.files.length || Object.keys(t.calls).length);
    const commands = worked.flatMap((t) => Object.values(t.calls)).filter((c) => /^(Bash|Shell|Terminal)$/.test(c.tool));
    return {
      worked,
      files: [...new Set(worked.flatMap((t) => t.files))],
      commands,
      failed: commands.filter((c) => c.state === "failed").length,
      last: [...worked].reverse().find((t) => t.gate),
    };
  }, [turns]);
  if (!l) return null;
  const gate = run?.state === "passed" || run?.state === "failed" ? run.state : last?.gate?.status;
  const blocker =
    l.running
      ? "The agent is mid-turn."
      : gate === "failed"
        ? "The project's checks fail on this tree."
        : !gate || gate === "none"
          ? "Nothing was checked: this project declares no checks, so 'done' is only a claim."
          : undefined;

  function check() {
    if (!ep) return;
    start<Check>(
      runKey,
      url(ep, "/api/verify", { wt: l!.wt }),
      ep.token,
      { state: "running", problems: [] },
      (next, e) =>
        e.event === "start"
          ? { ...next, command: String(e.data) }
          : e.event === "problem"
            ? { ...next, problems: [...next.problems, e.data as Problem] }
            : e.event === "none"
              ? { ...next, state: "none", why: String(e.data) }
              : e.event === "fatal"
                ? { ...next, state: "failed", why: String(e.data) }
                : e.event === "done"
                  ? { ...next, state: next.state === "none" ? "none" : String(e.data) === "0" ? "passed" : "failed" }
                  : next,
      (r) => (r.state === "running" ? { ...r, state: "failed", why: "The check stopped before it finished." } : r),
    );
  }

  function payload() {
    return {
      schema: 1,
      taskID: l!.id,
      title: l!.title,
      provider: l!.agent,
      repository: projectName,
      branch: l!.git?.branch ?? (l!.wt ? `keel/${l!.wt}` : null),
      worktree: l!.wt ?? null,
      head: [...worked].reverse().find((t) => t.commit)?.commit ?? null,
      files,
      commands: commands.map((c) => ({ tool: c.tool, command: String((c.input as { command?: string } | undefined)?.command ?? c.subject), reason: c.reason ?? null, failed: c.state === "failed" })),
      gates: worked.map((t: Turn) => ({ status: t.gate?.status ?? "none", command: t.gate?.command ?? null })),
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
      `- Checks: ${gate ?? "not run"}${blocker ? ` — ${blocker}` : ""}`,
      ``,
      `## Files changed (${files.length})`,
      ...files.map((f) => `- \`${f}\``),
      ``,
      `## Commands (${commands.length}${failed ? `, ${failed} failed` : ""})`,
      ...p.commands.slice(0, 100).map((c) => `- ${c.failed ? "✗" : "✓"} \`${c.command.split("\n")[0].slice(0, 160)}\``),
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
            <button className="primary" disabled={!!blocker && gate === "failed"} onClick={() => ask("merge")}>
              Merge…
            </button>
            <button onClick={() => ask("discard")}>Discard…</button>
          </>
        )}
      </div>

      <h3 className="eyebrow" style={{ margin: "18px 0 6px" }}>The project's checks</h3>
      <div className="actions" style={{ marginTop: 0 }}>
        <button onClick={check} disabled={run?.state === "running" || !ep}>
          {run?.state === "running" ? "Running…" : "Run the checks now"}
        </button>
        {run?.command && <code className="small">{run.command}</code>}
      </div>
      {run && run.state !== "running" && (
        <p className="small" style={{ color: run.state === "passed" ? "var(--add)" : run.state === "none" ? "var(--warn)" : "var(--del)" }}>
          {run.state === "passed" ? "Passed on the tree as it stands." : run.state === "none" ? (run.why ?? "No checks declared.") : (run.why ?? `Failed — ${run.problems.length} problem${run.problems.length === 1 ? "" : "s"}.`)}
        </p>
      )}
      {run?.problems.slice(0, 30).map((p, i) => (
        <div key={i} className="problem">
          <span className="del">✗</span>
          <span>{p.message}</span>
          <span className="where">
            {p.file}:{p.line}
          </span>
        </div>
      ))}
      {!run && (
        <p className="small muted">
          {worked.length} turn{worked.length === 1 ? "" : "s"} did work; the last recorded verdict is <strong>{last?.gate?.status ?? "none"}</strong>.
        </p>
      )}

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
        <button onClick={() => void navigator.clipboard.writeText(markdown()).then(() => setNote("Copied the summary as Markdown."))}>
          <Icon name="copy" size={14} /> Copy as Markdown
        </button>
        <button onClick={exportSigned}>Export signed packet…</button>
      </div>
      {note && <p className="small faint">{note}</p>}
    </div>
  );
}
