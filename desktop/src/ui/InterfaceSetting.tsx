import { useStore, type ChatInterface } from "../store";

/** Shared by first-run setup, project setup and Settings. Existing lanes retain their choice. */
export function InterfaceSetting({ applyToLane = false }: { applyToLane?: boolean }) {
  const value = useStore((s) => s.chatInterface);
  const active = useStore((s) => s.active);
  const surface = useStore((s) => s.active ? s.lanes[s.active]?.interface : undefined);
  const error = useStore((s) => s.active ? s.lanes[s.active]?.error : undefined);
  const busy = useStore((s) => !!s.active && !!(s.lanes[s.active]?.running || s.lanes[s.active]?.submitting || s.lanes[s.active]?.receipt));
  return <section className="settings-section interface-setting">
    <h2>Chat interface</h2>
    <p className="muted">Choose the default for new lanes. You can switch an idle lane from its menu.</p>
    <div className="interface-options" role="radiogroup" aria-label="Default chat interface">
      {([['chat', 'Formatted chat', 'Wrapped messages, code formatting, and a multiline composer.'], ['terminal', 'Terminal', 'The agent’s original interactive command-line interface.']] as const).map(([id, label, detail]) =>
        <label key={id}><input type="radio" name="chat-interface" checked={value === id} onChange={() => useStore.getState().setChatInterface(id as ChatInterface)} /><span><strong>{label}</strong><small>{detail}</small></span></label>)}
    </div>
    {applyToLane && active && surface !== value && <button disabled={busy} onClick={() => void useStore.getState().switchInterface(active, value)}>{busy ? 'Stop the current turn to switch' : 'Apply to current lane'}</button>}
    {applyToLane && error && <p className="error" role="alert">{error}</p>}
  </section>;
}
