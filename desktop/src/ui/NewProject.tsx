// New projects start Empty with an optional name and the shared, visible workspace destination.
import { UpdateButton } from "./UpdateButton";
import { useEffect, useState } from "react";
import { useProjectCreation, projectScratch, projectDestination } from "../project-creation";
import { open } from "@tauri-apps/plugin-dialog";
import { get, post } from "../api";
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
  { id: "empty", label: "Empty", detail: "A Git repository with Keel’s agent team and checklist." },
  { id: "app", label: "Cloudflare app", detail: "Next.js frontend and Hono API, each a Worker, joined by a service binding." },
  { id: "stack", label: "Container stack", detail: "Next.js and Hono on Node, Postgres and Redis, kustomize for dev and prod." },
] as const;

export function NewProject() {
  const [mode, setMode] = useState<"template" | "clone">("template");
  const creation = useProjectCreation();
  const { parent, setParent, busy, error, setError, created, run, reopen, resetOperation } = creation;
  const [name, setName] = useState("");
  const [template, setTemplate] = useState<(typeof TEMPLATES)[number]["id"]>("empty");
  const [repos, setRepos] = useState<Repo[]>();
  const [filter, setFilter] = useState("");
  const close = () => useStore.setState({ creating: false });

  useEffect(() => {
    if (mode !== "clone" || repos) return;
    let cleanup = () => {};
    projectScratch()
      .then(({ ep, done }) => {
        cleanup = done;
        return get<Repo[]>(ep, "/api/github/repos");
      })
      .then(setRepos, (e) => setError(/connect GitHub/.test(String(e)) ? "GitHub is not connected. Run `gh auth login` in a terminal, then come back." : String(e)))
      .finally(() => cleanup());
  }, [mode, repos, setError]);

  async function pick() {
    const dir = await open({ directory: true, title: "Where the project goes" });
    if (typeof dir === "string") { setParent(dir); resetOperation(); }
  }

  const valid = !name.trim() || /^[a-z0-9][a-z0-9-]{0,40}$/.test(name.trim());
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
          onChange={(value) => { if (!busy && !created) setMode(value); }}
        />
        <section className="settings-section">
          <p>
            Location <code className="project-location">{parent ?? "Finding your Keel projects folder…"}</code> <button disabled={!!busy || !!created} onClick={pick}>Change…</button>
          </p>
          {error && <div className="error" role="alert">{error}</div>}
          {created ? <><p>Project created at <code className="project-location">{created}</code>.</p><button className="primary" disabled={!!busy} onClick={() => void reopen()}>Open project</button></> : mode === "template" ? (
            <>
              <label className="field">
                Name (optional)
                <input value={name} disabled={!!busy} onChange={(e) => { setName(e.target.value.toLowerCase()); resetOperation(); }} placeholder="Automatic name" aria-describedby="project-name-help" aria-invalid={!!name.trim() && !valid} />
              </label>
              <p id="project-name-help" className="small muted">Leave blank and Keel will create a uniquely named project folder.</p>
              {name.trim() && !valid && <div className="small error">Use lowercase letters, digits and hyphens, up to 41 characters.</div>}
              <p className="small muted project-location">{name.trim() && valid ? projectDestination(parent ?? "", name) : "A unique project folder will be created in this location."}</p>
              <div className="templates">
                {TEMPLATES.map((t) => (
                  <button key={t.id} className={`template ${template === t.id ? "active" : ""}`} aria-pressed={template === t.id} disabled={!!busy} onClick={() => { setTemplate(t.id); resetOperation(); }}>
                    <strong>{t.label}</strong>
                    <span className="small muted">{t.detail}</span>
                  </button>
                ))}
              </div>
              <button
                className="primary"
                disabled={!valid || !parent || !!busy}
                onClick={() => run("create", (ep, operation) => post<{ path: string }>(ep, "/api/project/new", { name: name.trim(), parent, template, operation }).then((r) => r.path))}
              >
                {busy === "create" ? "Creating project…" : "Create project"}
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
