// Diff navigation preserves edits while saved correction anchors retain the original displayed range.
import { SyntaxText } from "./SyntaxText";
import {diffCommand} from "../delivery";
import { Virtuoso } from "react-virtuoso";
// Diff navigation preserves terminal mounts while corrections retain their original selected ranges.
import { useEffect, useRef, useState } from "react";
import { DiffCorrection, type DiffRange } from "./DiffCorrection";
import { useStore } from "../store";
import { Banner, focusTerminal } from "./kit";
import { inArea, type HunkRow } from "../git";
import { HunkActions } from "./HunkActions";
import { Icon } from "./icons";

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
export function Diff({ lane, embedded = false }: { lane: string; embedded?: boolean }) {
  const [range, setRange] = useState<DiffRange>(), [correcting, setCorrecting] = useState(false);
  const [actError, setActError] = useState<string>();
  const viewing = useStore((s) => s.lanes[lane]?.viewing);
  const diff = useStore((s) => s.lanes[lane]?.diff);
  const error = useStore((s) => s.lanes[lane]?.diffError);
  const staged = useStore((s) => s.lanes[lane]?.viewingStaged);
  const changes = useStore((s) => s.lanes[lane]?.git?.changes);
  const files = (changes ?? []).filter((c) => !c.dir && (staged === undefined || inArea(c, staged))).map((c) => c.path);
  const position = files.indexOf(viewing ?? "");
  const openDiff = useStore((s) => s.openDiff);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => box.current?.focus({ preventScroll: true }), [viewing]);
  useEffect(() => { setRange(undefined); setCorrecting(false); setActError(undefined); }, [viewing, staged]);
  function choose(side: DiffRange["side"], line: number, extend: boolean) {
    setRange((before) => extend && before?.side === side ? { side, first: Math.min(before.first, line), last: Math.max(before.last, line) } : { side, first: line, last: line }); setCorrecting(true);
  }
  const back = () => {
    openDiff(lane, undefined);
    requestAnimationFrame(() => {
      const row = viewing && document.querySelector<HTMLElement>(`.side-panel [data-file="${CSS.escape(viewing)}"][data-area="${staged ? "staged" : "changes"}"], .side-panel [data-file="${CSS.escape(viewing)}"]`);
      if (row) row.focus();
      else focusTerminal();
    });
  };
  const step = (by: number) => {
    const next = files[files.indexOf(viewing ?? "") + by];
    if (next) openDiff(lane, next, staged);
  };
  function keys(e: React.KeyboardEvent<HTMLDivElement>) {
    const command=diffCommand({...e,editable:!!(e.target as Element).closest("input,textarea,select,[contenteditable=true]"),correcting});
    if(!command)return;
    const commands={back,cancel:()=>{setCorrecting(false);box.current?.focus();},previous:()=>step(-1),next:()=>step(1)};
    commands[command]();
    e.preventDefault();
    e.stopPropagation();
  }
  return (
    <div className={`changes${embedded ? " scm-diff" : ""}`} ref={box} tabIndex={-1} onKeyDown={keys} style={{ outline: "none" }}>
      <div className="panel-head">
        <button className="link" onClick={back} title="Back to the list (Esc)">
          {embedded ? "Close diff" : "← Changes"} <kbd>Esc</kbd>
        </button>
        <span className="change-path" title={viewing}>
          {viewing?.split("/").at(-1)}
        </span>
        {diff && (
          <span className="small">
            <span className="add">+{diff.adds}</span> <span className="del">−{diff.dels}</span>
          </span>
        )}
        <div className="scm-diff-navigation">
          <button disabled={!diff?.identity} onClick={() => setCorrecting((open) => !open)}>Add correction</button>
          <button className="ghost" aria-label="Previous changed file" title="Previous changed file ([)" disabled={position <= 0} onClick={() => step(-1)}><Icon name="chevron-right" size={13} style={{ transform: "rotate(-90deg)" }} /></button>
          <button className="ghost" aria-label="Next changed file" title="Next changed file (])" disabled={position < 0 || position >= files.length - 1} onClick={() => step(1)}><Icon name="chevron-down" size={13} /></button>
        </div>
      </div>
      {correcting && diff && viewing && <DiffCorrection lane={lane} path={viewing} diff={diff} range={range} setRange={setRange} close={() => setCorrecting(false)} />}
      {staged !== undefined && <div className="scm-diff-path" title={viewing}>{staged ? "Staged Changes" : "Changes"}<span> / {viewing}</span></div>}
      {error ? <div className="panel-empty error" role="alert">{error}<button onClick={() => openDiff(lane, viewing, staged)}>Retry</button></div> : !diff ? (
        <div className="panel-empty muted">Reading the diff…</div>
      ) : diff.rows.length === 0 ? (
        <div className="panel-empty muted">{diff.note ?? "No textual difference."}</div>
      ) : (
        <>
          {actError && <Banner tone="error">{actError}</Banner>}
          {diff.note && <div className="small muted panel-note">{diff.note}</div>}
          <Virtuoso
            className="diff"
            totalCount={diff.rows.length}
            itemContent={(i) => {
              const r = diff.rows[i];
              if (r.kind === "hunk") {
                const h = r as HunkRow;
                return <div className="row hunk"><span className="hunk-header">{h.text}</span>{h.id && viewing && <HunkActions lane={lane} path={viewing} hunk={h} onError={setActError} reload={() => openDiff(lane, viewing, staged)} />}</div>;
              }
              const line = r as { kind: string; old: number | null; new: number | null; text: string };
              return (
                <div className={`row ${line.kind}`}>
                  {line.old ? <button className="ln" aria-label={`Correct old line ${line.old}`} aria-pressed={range?.side === "old" && line.old >= range.first && line.old <= range.last} onClick={(event) => choose("old", line.old!, event.shiftKey)}>{line.old}</button> : <span className="ln" />}
                  {line.new ? <button className="ln" aria-label={`Correct new line ${line.new}`} aria-pressed={range?.side === "new" && line.new >= range.first && line.new <= range.last} onClick={(event) => choose("new", line.new!, event.shiftKey)}>{line.new}</button> : <span className="ln" />}
                  <span className="code-text"><SyntaxText text={line.text} path={viewing ?? ""} /></span>
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
  if (project?.error) return <div className="panel-empty error">Oya Factory could not start for this project: {project.error}</div>;
  if (!project?.endpoint) return <div className="panel-empty muted">Starting Oya Factory for this project…</div>;
  return <div className="panel-empty muted">Reading git…</div>;
}
