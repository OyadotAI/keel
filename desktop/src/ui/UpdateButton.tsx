import { useState } from "react";
import { restart, useUpdate } from "../update";
import { useStore } from "../store";
import { Dialog } from "./kit";

export function UpdateButton() {
  const u = useUpdate();
  const running = useStore((s) => Object.values(s.lanes).filter((l) => l.running).length);
  const [asking, setAsking] = useState(false);
  if (u.state === "failed" && u.why.startsWith("The update could not install"))
    return (
      <button className="bad" onClick={() => useStore.setState({ settings: true })} title={u.why}>
        Update failed — see Settings
      </button>
    );
  if (u.state === "installing") return <button className="update" disabled aria-live="polite">Installing…</button>;
  if (u.state !== "ready") return null;
  return (
    <>
      <button className="update" onClick={() => (running ? setAsking(true) : void restart())} title={`Keel ${u.version} is downloaded`}>
        Update now
      </button>
      {asking && (
        <Dialog title={`Restart to install Keel ${u.version}?`} onClose={() => setAsking(false)}>
          <p>
            {running} lane{running === 1 ? " is" : "s are"} running a turn, and restarting stops {running === 1 ? "it" : "them"}. The conversations stay in History and resume after the restart.
          </p>
          <div className="actions">
            <button className="primary" onClick={() => void restart()}>
              Restart now
            </button>
            <button onClick={() => setAsking(false)}>Later</button>
          </div>
        </Dialog>
      )}
    </>
  );
}
