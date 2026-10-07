import { UpdateButton } from "./UpdateButton";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { get, post, type Endpoint } from "../api";
import { useStore } from "../store";
import { InterfaceSetting } from "./InterfaceSetting";
import { Tabs } from "./kit";

interface Repo {
  full_name: string;
  name: string;
  private: boolean;
  description?: string | null;
  clone_url: string;
  updated_at?: string | null;
}

const TEMPLATES = [
  { id: "app", label: "Cloudflare app", detail: "Next.js frontend and Hono API, each a Worker, joined by a service binding." },
  { id: "stack", label: "Container stack", detail: "Next.js and Hono on Node, Postgres and Redis, kustomize for dev and prod." },
  { id: "empty", label: "Empty", detail: "A git repository with Keel's agent team and checklist, nothing else." },
] as const;

/// A project that does not exist yet needs a daemon that is not about a project. One runs on an
/// empty folder for the length of this page, makes or clones the project, and hands over.
async function scratch(): Promise<{ ep: Endpoint; done: () => void }> {
  const dir = await invoke<string>("scratch_dir");
  const ep = await invoke<Endpoint>("open_project", { path: dir });
  return { ep, done: () => void invoke("close_project", { path: dir }) };
}

export function NewProject() {
  const [mode, setMode] = useState<"template" | "clone">("template");
  const [parent, setParent] = useState<string>();
  const [name, setName] = useState("");
  const [template, setTemplate] = useState<(typeof TEMPLATES)[number]["id"]>("app");
  const [repos, setRepos] = useState<Repo[]>();
  const [filter, setFilter] = useState("");
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();
  const close = () => useStore.setState({ creating: false });

  useEffect(() => {
    if (mode !== "clone" || repos) return;
    let cleanup = () => {};
    scratch()
      .then(({ ep, done }) => {
        cleanup = done;
        return get<Repo[]>(ep, "/api/github/repos");
      })
      .then(setRepos, (e) => setError(/connect GitHub/.test(String(e)) ? "GitHub is not connected. Run `gh auth login` in a terminal, then come back." : String(e)))
      .finally(() => cleanup());
  }, [mode, repos]);

  async function pick() {
    const dir = await open({ directory: true, title: "Where the project goes" });
    if (typeof dir === "string") setParent(dir);
  }

  async function run(what: string, act: (ep: Endpoint) => Promise<string>) {
    if (!parent) return setError("Choose the folder it goes in first.");
    setBusy(what);
    setError(undefined);
    let done = () => {};
    try {
      const s = await scratch();
      done = s.done;
      const path = await act(s.ep);
      done();
      done = () => {};
      close();
      await useStore.getState().openProject(path);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      done();
      setBusy(undefined);
    }
  }

  const valid = /^[a-z0-9][a-z0-9-]{0,40}$/.test(name);
  const shown = (repos ?? []).filter((r) => r.full_name.toLowerCase().includes(filter.toLowerCase())).slice(0, 100);

  return (
    <div className="settings">
      <header className="lane-header">
        <div className="lane-heading">
          <span className="lane-name">New project</span>
          <span className="muted small">Start one from a template, or clone one of yours from GitHub</span>
        </div>
        <UpdateButton />
        <button onClick={close}>Cancel</button>
      </header>
      <div className="settings-body">
        <InterfaceSetting />
        <Tabs
          tabs={[
            { id: "template", label: "From a template" },
            { id: "clone", label: "Clone from GitHub" },
          ]}
          value={mode}
          onChange={setMode}
        />
        <section className="settings-section">
          <p>
            Goes in <code>{parent ?? "…"}</code> <button onClick={pick}>{parent ? "Change…" : "Choose a folder…"}</button>
          </p>
          {error && <div className="error">{error}</div>}
          {mode === "template" ? (
            <>
              <label className="field">
                Name
                <input value={name} onChange={(e) => setName(e.target.value.toLowerCase())} placeholder="my-app" aria-invalid={!!name && !valid} />
              </label>
              {name && !valid && <div className="small error">Lowercase letters, digits and hyphens — it becomes a folder and two Worker names.</div>}
              <div className="templates">
                {TEMPLATES.map((t) => (
                  <button key={t.id} className={`template ${template === t.id ? "active" : ""}`} aria-pressed={template === t.id} onClick={() => setTemplate(t.id)}>
                    <strong>{t.label}</strong>
                    <span className="small muted">{t.detail}</span>
                  </button>
                ))}
              </div>
              <button
                className="primary"
                disabled={!valid || !parent || !!busy}
                onClick={() => run("create", (ep) => post<{ path: string }>(ep, "/api/project/new", { name, parent, template }).then((r) => r.path))}
              >
                {busy === "create" ? "Creating…" : "Create project"}
              </button>
            </>
          ) : !repos ? (
            !error && <p className="muted">Reading your repositories…</p>
          ) : (
            <>
              <input className="field" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Filter" aria-label="Filter repositories" />
              <div className="repos">
                {shown.map((r) => (
                  <button
                    key={r.full_name}
                    className="lane"
                    disabled={!!busy || !parent}
                    title={r.description ?? undefined}
                    onClick={() =>
                      run(r.full_name, (ep) =>
                        post<{ path: string }>(ep, "/api/github/clone", { clone_url: r.clone_url, name: r.name, parent }).then((x) => x.path),
                      )
                    }
                  >
                    <span className="lane-title">{r.full_name}</span>
                    <span className="muted small">{busy === r.full_name ? "cloning…" : r.private ? "private" : ""}</span>
                  </button>
                ))}
              </div>
            </>
          )}
        </section>
      </div>
    </div>
  );
}
