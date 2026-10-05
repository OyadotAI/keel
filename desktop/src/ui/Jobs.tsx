import { useState } from "react";
import { useStore, type Job } from "../store";

/// Commands the daemon runs for this lane, which outlive the turn that started them. Running and
/// visible is the point: nobody was asked, so this is where it is answered for.
export function Jobs({ lane }: { lane: string }) {
  const jobs = useStore((s) => s.lanes[lane]?.jobs);
  if (!jobs?.length)
    return (
      <div className="panel-empty muted">
        Nothing running in the background. When the agent starts a long command — a dev server, a watcher, `gh run watch` — Keel runs it here so it outlives the turn, and hands the result back when it ends.
      </div>
    );
  return (
    <div className="jobs">
      {jobs.map((j) => (
        <JobRow key={j.id} lane={lane} job={j} />
      ))}
    </div>
  );
}

function JobRow({ lane, job }: { lane: string; job: Job }) {
  const stop = useStore((s) => s.stopJob);
  const [open, setOpen] = useState(!job.finished);
  const running = !job.finished;
  return (
    <div className="job">
      <div className="job-line">
        <span className={`mark ${running ? "running" : job.exit === 0 ? "ok" : "failed"}`} aria-label={running ? "Running" : `Exited ${job.exit}`} role="img" />
        <button className="link job-command" onClick={() => setOpen(!open)} aria-expanded={open} title={job.dir}>
          {job.command}
        </button>
        {running ? (
          <button onClick={() => stop(lane, job.id)}>Stop</button>
        ) : (
          <span className="small muted">exit {job.exit ?? "?"}</span>
        )}
      </div>
      {open && <pre className="code">{job.log.slice(-40).join("\n") || "(no output yet)"}</pre>}
    </div>
  );
}
