import { Virtuoso } from "react-virtuoso";
import { useEffect, useRef } from "react";
import { useStore } from "../store";
import { focusTerminal } from "./kit";

/// What git says moved in this lane's checkout. Read on events, never on a clock.
export function Changes({ lane }: { lane: string }) {
  const git = useStore((s) => s.lanes[lane]?.git);
  const viewing = useStore((s) => s.lanes[lane]?.viewing);
  const openDiff = useStore((s) => s.openDiff);
  if (viewing) return <Diff lane={lane} />;
  if (!git) return <Waiting lane={lane} />;
  if (!git.is_repo) return <div className="panel-empty muted">This folder is not a git repository, so there is nothing to compare against.</div>;
  if (git.changes.length === 0) return <div className="panel-empty muted">No changes on {git.branch ?? "this branch"}.</div>;
  return (
    <div className="changes">
      <div className="panel-head small muted">
        {git.changes.length} changed on {git.branch ?? "a detached HEAD"}
        {git.collapsed && " — some folders collapsed"}
      </div>
      <Virtuoso
        className="changes-list"
        totalCount={git.changes.length}
        itemContent={(i) => {
          const c = git.changes[i];
          return (
            <button data-file={c.path} className="change" onClick={() => !c.dir && openDiff(lane, c.path)} title={c.label} disabled={c.dir}>
              <span className={`status s-${c.status.trim().slice(0, 1).toLowerCase() || "m"}`}>{c.status.trim() || "M"}</span>
              <span className="change-path">{c.path}</span>
            </button>
          );
        }}
      />
    </div>
  );
}

/// Esc goes back to the list with this file focused; ] and [ step through the changed files, so a
/// review is read without the mouse.
function Diff({ lane }: { lane: string }) {
  const viewing = useStore((s) => s.lanes[lane]?.viewing);
  const diff = useStore((s) => s.lanes[lane]?.diff);
  const openDiff = useStore((s) => s.openDiff);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => box.current?.focus({ preventScroll: true }), [viewing]);
  const back = () => {
    openDiff(lane, undefined);
    requestAnimationFrame(() => {
      const row = viewing && document.querySelector<HTMLElement>(`.side-panel [data-file="${CSS.escape(viewing)}"]`);
      if (row) row.focus();
      else focusTerminal();
    });
  };
  const step = (by: number) => {
    const files = (useStore.getState().lanes[lane]?.git?.changes ?? []).filter((c) => !c.dir).map((c) => c.path);
    const next = files[files.indexOf(viewing ?? "") + by];
    if (next) openDiff(lane, next);
  };
  function keys(e: React.KeyboardEvent<HTMLDivElement>) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "Escape") back();
    else if (e.key === "]") step(1);
    else if (e.key === "[") step(-1);
    else return;
    e.preventDefault();
    e.stopPropagation();
  }
  return (
    <div className="changes" ref={box} tabIndex={-1} onKeyDown={keys} style={{ outline: "none" }}>
      <div className="panel-head">
        <button className="link" onClick={back} title="Back to the list (Esc)">
          ← Changes <kbd>Esc</kbd>
        </button>
        <span className="change-path" title={viewing}>
          {viewing}
        </span>
        {diff && (
          <span className="small">
            <span className="add">+{diff.adds}</span> <span className="del">−{diff.dels}</span>
          </span>
        )}
        <span className="small faint" title="Previous and next changed file">
          <kbd>[</kbd> <kbd>]</kbd>
        </span>
      </div>
      {!diff ? (
        <div className="panel-empty muted">Reading the diff…</div>
      ) : diff.rows.length === 0 ? (
        <div className="panel-empty muted">{diff.note ?? "No textual difference."}</div>
      ) : (
        <>
          {diff.note && <div className="small muted panel-note">{diff.note}</div>}
          <Virtuoso
            className="diff"
            totalCount={diff.rows.length}
            itemContent={(i) => {
              const r = diff.rows[i];
              if (r.kind === "hunk") return <div className="row hunk">{"text" in r ? r.text : ""}</div>;
              const line = r as { kind: string; old: number | null; new: number | null; text: string };
              return (
                <div className={`row ${line.kind}`}>
                  <span className="ln">{line.old ?? ""}</span>
                  <span className="ln">{line.new ?? ""}</span>
                  <span className="code-text">{line.text}</span>
                </div>
              );
            }}
          />
        </>
      )}
    </div>
  );
}

/// Says which of the reasons it is: the daemon starting, the daemon failing, or git being read.
function Waiting({ lane }: { lane: string }) {
  const project = useStore((s) => s.projects[s.lanes[lane]?.project ?? ""]);
  if (project?.error) return <div className="panel-empty error">Keel could not start for this project: {project.error}</div>;
  if (!project?.endpoint) return <div className="panel-empty muted">Starting Keel for this project…</div>;
  return <div className="panel-empty muted">Reading git…</div>;
}
