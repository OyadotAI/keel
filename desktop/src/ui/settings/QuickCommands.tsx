// Settings › Quick commands: the project's saved shell commands, added and removed here and run
// from the palette. Stored on this machine (quick-commands.ts), never in the repository.
import { useState } from "react";
import { addQuickCommand, quickCommands, removeQuickCommand } from "../../quick-commands";
import { Banner } from "../kit";

/// The list and the add row for one project. Nothing is shown without a project to keep them for.
export function QuickCommands({ project }: { project?: string }) {
  const [list, setList] = useState(() => (project ? quickCommands(project) : []));
  const [name, setName] = useState("");
  const [command, setCommand] = useState("");
  const [error, setError] = useState<string>();
  if (!project) return null;
  const change = (f: () => void) => {
    try {
      f();
      setError(undefined);
      setList(quickCommands(project));
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : `Couldn't save quick commands. ${String(e)}`);
      return false;
    }
  };
  const add = () => {
    if (change(() => addQuickCommand(project, { name, command }))) { setName(""); setCommand(""); }
  };
  return (
    <section className="settings-section" aria-labelledby="settings-quick-commands">
      <h2 id="settings-quick-commands">Quick commands</h2>
      {list.length ? (
        <ul className="rules">
          {list.map((c) => (
            <li key={c.name}><strong>{c.name}</strong><code>{c.command}</code><button className="link" aria-label={`Remove ${c.name}`} onClick={() => change(() => removeQuickCommand(project, c.name))}>Remove</button></li>
          ))}
        </ul>
      ) : <p className="muted">No quick commands yet. Add one to run it from the palette (⌘K) in this project's shell.</p>}
      <form className="field" onSubmit={(e) => { e.preventDefault(); add(); }}>
        <input aria-label="Name" placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} />
        <input aria-label="Command" placeholder="Command" value={command} onChange={(e) => setCommand(e.target.value)} style={{ flex: 2, fontFamily: "var(--mono)" }} />
        <button type="submit" disabled={!name.trim() || !command.trim()}>Add</button>
      </form>
      {error && <Banner tone="error">{error}</Banner>}
    </section>
  );
}
