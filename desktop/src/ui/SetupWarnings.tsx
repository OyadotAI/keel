import { useState } from "react";
import { useStore } from "../store";
import type { Warning } from "../setup";
import { Floating } from "./Menu";
import { Icon } from "./icons";

const FIX: Record<Warning["fix"], string> = { settings: "Open Settings", extensions: "Open Extensions", readiness: "Open Readiness" };

/// What is missing from a project's agent setup, and the one place each is fixed.
export function SetupList({ project, close, showTab }: { project: string; close: () => void; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]) ?? [];
  const go = (w: Warning) => {
    close();
    if (w.fix === "settings") useStore.setState({ settings: true, extensions: false, creating: false });
    else if (w.fix === "extensions") useStore.setState({ extensions: true, settings: false, creating: false });
    else {
      useStore.setState({ settings: false, extensions: false, creating: false });
      showTab?.("readiness");
    }
  };
  if (!warnings.length) return <div className="small muted" style={{ padding: 8 }}>Nothing missing in this project's setup.</div>;
  return (
    <div style={{ maxHeight: 360, overflowY: "auto" }}>
      <div className="eyebrow" style={{ padding: "4px 4px 8px" }}>
        Setup · {warnings.length} to look at
      </div>
      {warnings.map((w) => (
        <div key={w.id} style={{ padding: "8px 4px", borderTop: "1px solid var(--line)" }}>
          <div style={{ display: "flex", gap: 8, alignItems: "baseline" }}>
            <span style={{ color: w.severity === "critical" ? "var(--del)" : "var(--warn)", fontWeight: 600, fontSize: 11 }}>{w.severity === "critical" ? "Blocks lanes" : "Missing"}</span>
            <span style={{ fontWeight: 500, flex: 1 }}>{w.title}</span>
          </div>
          <div className="small muted" style={{ margin: "2px 0 6px" }}>{w.detail}</div>
          <button className="link small" style={{ padding: 0 }} onClick={() => go(w)}>
            {FIX[w.fix]} →
          </button>
        </div>
      ))}
    </div>
  );
}

/// The count, where a project is named: quiet when nothing is missing, one click to the list.
export function SetupBadge({ project, showTab }: { project: string; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]);
  const [at, setAt] = useState<DOMRect | null>(null);
  if (!warnings?.length) return null;
  const blocking = warnings.some((w) => w.severity === "critical");
  return (
    <>
      <button
        className="setup-badge"
        style={{ color: blocking ? "var(--del)" : "var(--warn)" }}
        onClick={(e) => {
          e.stopPropagation();
          setAt(e.currentTarget.getBoundingClientRect());
        }}
        title={`${warnings.length} thing${warnings.length === 1 ? "" : "s"} missing from this project's setup`}
        aria-label={`${warnings.length} setup warnings`}
      >
        <Icon name="octagon" size={13} /> {warnings.length} to set up
      </button>
      {at && (
        <Floating anchor={at} kind="popover" width={360} onClose={() => setAt(null)}>
          <SetupList project={project} close={() => setAt(null)} showTab={showTab} />
        </Floating>
      )}
    </>
  );
}
