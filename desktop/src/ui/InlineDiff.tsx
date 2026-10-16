import { SyntaxText } from "./SyntaxText";
import type { Prepared } from "../git";

/// A short diff drawn in place — a turn's file, a commit's file. The full pane is `Changes`; this
/// is the first `limit` rows, because a lockfile is a normal thing for an agent to write.
export function InlineDiff({ rows, path = "", limit = 400 }: { rows: Prepared["rows"]; path?: string; limit?: number }) {
  return (
    <div className="inline-diff">
      {rows.slice(0, limit).map((r, i) =>
        r.kind === "hunk" ? (
          <div key={i} className="row hunk">
            {"text" in r ? r.text : ""}
          </div>
        ) : (
          <div key={i} className={`row ${r.kind}`}>
            <span className="ln">{"old" in r ? (r.old ?? "") : ""}</span>
            <span className="ln">{"new" in r ? (r.new ?? "") : ""}</span>
            <span className="code-text"><SyntaxText text={"text" in r ? r.text : ""} path={path} /></span>
          </div>
        ),
      )}
    </div>
  );
}
