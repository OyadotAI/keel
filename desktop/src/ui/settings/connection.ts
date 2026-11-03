// Project-tagged connection results cannot appear beneath a different project's identity.
import { useEffect, useState } from "react";
import type { Endpoint } from "../../api";
import { useStore } from "../../store";

interface Connection { project?: string; endpoint?: Endpoint; error?: string }

/** Keep preferences available while connecting, and discard results for a project already left. */
export function useProjectConnection(project?: string) {
  const [state, setState] = useState<Connection>({});
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let live = true;
    setState({ project });
    if (project) void useStore.getState().ensure(project).then(
      (endpoint) => { if (live) setState({ project, endpoint }); },
      (error) => { if (live) setState({ project, error: String(error) }); },
    );
    return () => { live = false; };
  }, [project, attempt]);
  const current = state.project === project ? state : {};
  return { ...current, retry: () => setAttempt((n) => n + 1) };
}
