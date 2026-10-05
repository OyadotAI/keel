import { openUrl } from "@tauri-apps/plugin-opener";
import { useStore } from "../store";
import { frameable } from "../api";

/// The project's dev server, and the page it serves. One server for the project: a lane that is
/// not the one running it is told whose it is, rather than shown another lane's tree as its own.
export function Preview({ lane }: { lane: string }) {
  const dev = useStore((s) => s.lanes[lane]?.dev);
  const wt = useStore((s) => s.lanes[lane]?.wt);
  const toggle = useStore((s) => s.dev);
  if (!dev) return <div className="panel-empty muted">Asking the daemon about the dev server…</div>;
  if (dev.running && dev.elsewhere)
    return (
      <div className="panel-empty muted">
        The dev server is running for {dev.owner ? `the lane on keel/${dev.owner}` : "the project's own tree"}, not this one{wt ? "" : " — this lane shares the project tree"}. One runs at a time.
      </div>
    );
  if (!dev.running)
    return (
      <div className="panel-empty">
        <p className="muted">{dev.detected ? <>Not running. Keel would start it with <code>{dev.detected}</code>.</> : "No dev server found in this project."}</p>
        {dev.detected && (
          <button className="primary" onClick={() => toggle(lane, true)}>
            Start the dev server
          </button>
        )}
      </div>
    );
  return (
    <div className="preview">
      <div className="panel-head small">
        <span className="change-path">{dev.url ?? "Starting…"}</span>
        <button onClick={() => toggle(lane, false)}>Stop</button>
      </div>
      {dev.url && frameable(dev.url) ? (
        <iframe className="preview-frame" src={frameable(dev.url)!} title="Preview" />
      ) : dev.url ? (
        <div className="panel-empty">
          <p className="muted">This server is not on this machine's own address, so it cannot be shown inside Keel.</p>
          <button onClick={() => openUrl(dev.url!)}>Open {dev.url} in the browser</button>
        </div>
      ) : (
        <pre className="code">{dev.log.slice(-30).join("\n") || "Waiting for it to say where it is listening…"}</pre>
      )}
    </div>
  );
}
