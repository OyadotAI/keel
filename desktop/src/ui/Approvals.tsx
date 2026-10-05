import { useState } from "react";
import { useStore, type Pending } from "../store";
import { Icon } from "./icons";

interface Question {
  question: string;
  options?: { label: string; description?: string }[];
}

/// A refused command is a question, docked under the terminal so it neither hides the CLI's own
/// pending-tool line nor lives in a panel that can be closed. Focus is never taken from the
/// terminal: ⌘⇧A and ⌘⇧D answer the oldest from anywhere.
export function ApprovalStrip({ lane }: { lane: string }) {
  const pending = useStore((s) => s.lanes[lane]?.pending);
  if (!pending?.length) return null;
  return (
    <div className="strip" role="region" aria-label="Waiting for your answer">
      {pending.map((p, i) => (
        <Row key={p.id} lane={lane} p={p} count={i === 0 ? pending.length : 0} />
      ))}
    </div>
  );
}

function Row({ lane, p, count }: { lane: string; p: Pending; count: number }) {
  const answer = useStore((s) => s.answer);
  const [all, setAll] = useState(false);
  if (p.tool === "AskUserQuestion") return <Questions lane={lane} p={p} />;
  const lines = p.command.split("\n");
  return (
    <div className="strip-row">
      <div className="strip-title">
        <Icon name="hand" className="hand" />
        <span style={{ flex: 1 }}>The agent wants to run {p.tool === "Bash" ? "a command" : p.tool}</span>
        {count > 1 && <span className="small faint">{count} waiting</span>}
      </div>
      {p.command && (
        <pre className="code" style={{ maxHeight: all ? 320 : undefined }}>
          $ {all ? p.command : lines.slice(0, 6).join("\n")}
          {!all && lines.length > 6 && "\n…"}
        </pre>
      )}
      {!all && lines.length > 6 && (
        <button className="link small" onClick={() => setAll(true)}>
          Show all
        </button>
      )}
      <div className="actions">
        <button className="primary" onClick={() => answer(lane, p, "allow")}>
          Allow once <kbd>⌘⇧A</kbd>
        </button>
        <button onClick={() => answer(lane, p, "deny")}>
          Deny <kbd>⌘⇧D</kbd>
        </button>
      </div>
    </div>
  );
}

function Questions({ lane, p }: { lane: string; p: Pending }) {
  const answer = useStore((s) => s.answer);
  const questions = (p.input.questions as Question[] | undefined) ?? [];
  const [picked, setPicked] = useState<Record<number, string>>({});
  const complete = questions.length > 0 && questions.every((_, i) => picked[i] !== undefined);
  function send(chosen: Record<number, string>) {
    void answer(lane, p, "deny", questions.map((q, i) => `${q.question}: ${chosen[i]}`).join("\n"));
  }
  return (
    <div className="strip-row">
      <div className="strip-title">
        <Icon name="hand" className="hand" />
        The agent is asking
      </div>
      {questions.map((q, i) => (
        <div key={i} style={{ marginBottom: 8 }}>
          <div style={{ fontWeight: 500 }}>{q.question}</div>
          <div className="actions">
            {(q.options ?? []).map((o) => (
              <button
                key={o.label}
                title={o.description}
                className={picked[i] === o.label ? "primary" : undefined}
                aria-pressed={picked[i] === o.label}
                onClick={() => {
                  const next = { ...picked, [i]: o.label };
                  setPicked(next);
                  if (questions.length === 1) send(next);
                }}
              >
                {o.label}
              </button>
            ))}
          </div>
        </div>
      ))}
      {questions.length > 1 && (
        <button className="primary" disabled={!complete} onClick={() => send(picked)}>
          {complete ? "Send answers" : `Answer all ${questions.length} questions`}
        </button>
      )}
    </div>
  );
}
