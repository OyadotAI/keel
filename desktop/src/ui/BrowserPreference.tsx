// Browser selection is device-local and does not redirect an already-running session.
import { useState } from "react";
import "./BrowserPreference.css";
import { useBrowserPreference, type BrowserBackend } from "../browser-preference";

function Choice({ state }: { state: ReturnType<typeof useBrowserPreference> }) {
  const [selected, setSelected] = useState<BrowserBackend | "">(state.value?.selected ?? "");
  return <div className="browser-choice" role="group" aria-label="Where should agents browse?">
    <h3>Where should agents browse?</h3>
    <label><input type="radio" name="agent-browser" checked={selected === "embedded"} onChange={() => setSelected("embedded")} /> <strong>Inside Keel</strong><span>Keep pages inside Keel. Sign-ins stay with each project.</span></label>
    <label><input type="radio" name="agent-browser" checked={selected === "local_oya"} disabled={!state.value?.installation} onChange={() => setSelected("local_oya")} /> <strong>Local Oya Browser</strong><span>Use your installed Oya Browser and its existing signed-in profile.</span></label>
    <p className="muted small">Saved on this device for all projects and coding agents.</p>
    <div className="handoff-actions"><button className="primary" disabled={!selected || state.busy} onClick={() => selected && void state.save(selected)}>Save browser choice</button><button disabled={state.busy} onClick={state.defer}>Not now</button></div>
  </div>;
}
/** Show installation, connection and session state separately so a preference is not mistaken for readiness. */
export function BrowserPreference({ lane }: { lane?: string }) {
  const state = useBrowserPreference(lane);
  return <BrowserPreferenceView state={state} lane={lane} />;
}
export function BrowserPreferenceView({ state, lane }: { state: ReturnType<typeof useBrowserPreference>; lane?: string }) {
  const value = state.value;
  const names = { embedded: "Inside Keel", local_oya: "Local Oya Browser" };
  return <section className="settings-section browser-preference">
    <h2>Browser for coding agents</h2>
    {!value && !state.error && <p className="muted">Checking for Oya Browser…</p>}
    {value && <><p>Saved default: {value.selected ? names[value.selected] : "Inside Keel (default)"}.</p>
      <p className="muted small">{value.detection_error ?? value.connection_message}</p>
      {lane && value.selected && value.effective.backend !== value.selected && <p className="muted small">This session uses {names[value.effective.backend]}. {names[value.selected]} will be used for new sessions.</p>}
      <div className="handoff-actions"><button disabled={state.busy} onClick={state.choose}>Change…</button><button disabled={state.busy} onClick={() => void state.refresh()}>Check again</button></div>
    </>}
    {state.choosing && <Choice state={state} />}
    {state.error && <p className="error" role="alert">{state.error}</p>}
  </section>;
}
/** A local session exposes only the page it owns; personal tabs remain in Oya. */
export function LocalOyaView({ state }: { state: ReturnType<typeof useBrowserPreference> }) {
  const value = state.value;
  if (!value || value.effective.backend !== "local_oya") return null;
  return <div className="preview browser local-oya"><h2>Local Oya Browser</h2><p>{value.connection_message}</p>
    <p>{value.effective.url ?? "This session has not opened a page yet."}</p>
    <div className="handoff-actions"><button disabled={!value.effective.url || state.busy} onClick={() => void state.show()}>Show in Oya</button><button disabled={state.busy} onClick={() => void state.reconnect()}>Retry connection</button></div>
    {state.error && <p className="error" role="alert">{state.error}</p>}
  </div>;
}
