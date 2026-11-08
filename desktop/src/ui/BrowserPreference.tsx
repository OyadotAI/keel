// Where agents browse: each lane chooses, on its first browser step (the card) or in its Browser
// tab; Settings holds the device's default — ask each lane, or always one browser.
import "./BrowserPreference.css";
import { useBrowserPreference, type BrowserBackend } from "../browser-preference";

type State = ReturnType<typeof useBrowserPreference>;
const NAMES: Record<BrowserBackend, string> = { embedded: "Inside Keel", local_oya: "Oya Browser" };

/** Settings: the device default. "Ask each lane" is what a new person gets; lanes that chose keep it. */
export function BrowserPreference({ lane }: { lane?: string }) {
  const state = useBrowserPreference(lane);
  return <BrowserPreferenceView state={state} />;
}
export function BrowserPreferenceView({ state }: { state: State }) {
  const value = state.value;
  const option = (selected: BrowserBackend | null, title: string, detail: string, disabled = false) => (
    <label>
      <input type="radio" name="agent-browser" checked={value?.selected === selected} disabled={disabled || state.busy} onChange={() => void state.save(selected)} />{" "}
      <strong>{title}</strong>
      <span>{detail}</span>
    </label>
  );
  return <section className="settings-section browser-preference">
    <h2>Browser for coding agents</h2>
    {!value && !state.error && <p className="muted">Checking for Oya Browser…</p>}
    {value && <div className="browser-choice" role="radiogroup" aria-label="Browser for coding agents">
      {option(null, "Ask each lane (recommended)", "The first time an agent in a lane needs a browser, Keel asks where that lane should browse.")}
      {option("embedded", "Always Inside Keel", "A browser in each lane's Browser tab. Sign-ins stay with each project.")}
      {option("local_oya", "Always Oya Browser", value.installation ? "The Oya app on your computer, with the accounts you're already signed into." : "Oya Browser isn't installed. Choose “Ask each lane” and Keel offers to install it.", !value.installation)}
      <p className="muted small">Lanes that already chose keep their choice. Change one in its Browser tab.</p>
    </div>}
    {state.error && <p className="error" role="alert">{state.error}</p>}
  </section>;
}

/** The lane's own switch, in the Browser tab's status row. Applies to the lane's next browser step. */
export function BrowseIn({ state }: { state: State }) {
  const effective = state.value?.effective;
  if (!state.value) return null;
  const value = effective?.source === "none" ? "" : effective?.backend ?? "";
  return <label className="browse-in small">
    <span className="muted">Browse in</span>
    <select value={value} disabled={state.busy} onChange={(e) => e.target.value && void state.setLane(e.target.value as BrowserBackend)} aria-label="Where this lane browses">
      {value === "" && <option value="">Asks on first browser step</option>}
      <option value="embedded">{NAMES.embedded}</option>
      <option value="local_oya">{state.value.installation ? NAMES.local_oya : "Oya Browser (installs on next step)"}</option>
    </select>
  </label>;
}

/** A lane browsing in the person's own Oya: Keel shows only the page this lane owns there. */
export function LocalOyaView({ state }: { state: State }) {
  const value = state.value;
  if (!value || value.effective.backend !== "local_oya") return null;
  return <div className="preview browser local-oya">
    <div className="browser-status idle"><span className="browser-status-text">{value.connection_message}</span><BrowseIn state={state} /></div>
    <h2>This lane browses in your Oya Browser</h2>
    <p>{value.effective.url ?? "The agent hasn't opened a page yet."}</p>
    <div className="handoff-actions"><button disabled={!value.effective.url || state.busy} onClick={() => void state.show()}>Show in Oya</button><button disabled={state.busy} onClick={() => void state.reconnect()}>Retry connection</button></div>
    {state.error && <p className="error" role="alert">{state.error}</p>}
  </div>;
}
