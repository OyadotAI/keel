import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { EVENTS, groupHooks, tilde } from "../hooks";
import { useEffect, useState } from "react";
import { get, post, url, type Endpoint } from "../api";
import { start, useRun } from "../runs";
import { useStore } from "../store";
import { Icon } from "./icons";
import { Count, Tabs } from "./kit";

interface Named {
  name: string;
  description?: string | null;
  scope: string;
  path?: string;
  plugin?: string | null;
  generated?: boolean;
}
interface Hook {
  event: string;
  command: string;
  scope: string;
  source: string;
}
interface Mcp {
  name: string;
  endpoint?: string | null;
  scope: string;
  source: string;
}
interface InstalledPlugin {
  name: string;
  enabled: boolean;
  marketplace?: string | null;
  version?: string | null;
  scope?: string | null;
}
interface Workspace {
  skills: Named[];
  agents: Named[];
  commands: Named[];
  hooks: Hook[];
  mcp_servers: Mcp[];
  plugins: InstalledPlugin[];
}
interface CatalogSkill {
  id: string;
  description: string;
  author: string;
  present?: string | null;
}
interface CatalogPlugin {
  name: string;
  description: string;
  marketplace: string;
  installed: boolean;
  reason?: string | null;
}

type Section = "skills" | "agents" | "mcp" | "plugins" | "hooks" | "commands";

/// What Claude Code loads in this project — skills, subagents, MCP servers, plugins, hooks,
/// commands — read from Claude Code's own files, and the ways to add to it. Every write goes
/// through the daemon, which never writes over a file and commits only what it wrote.
export function Extensions() {
  const project = useStore((s) => (s.active ? s.lanes[s.active]?.project : undefined) ?? s.order[0]);
  const name = useStore((s) => (project ? s.projects[project]?.name : undefined));
  const [ep, setEp] = useState<Endpoint>();
  const [ws, setWs] = useState<Workspace>();
  const [section, setSection] = useState<Section>("skills");
  const [error, setError] = useState<string>();
  // An install is kept outside this page: closing Extensions mid-install, or opening it again,
  // shows the same run rather than an idle button that would start a second one.
  const runKey = `ext:${project ?? ""}`;
  const run = useRun<{ what: string; log: string[] }>(runKey);
  const running = run?.running ? run.state.what : undefined;
  const log = run?.state.log ?? [];

  const read = (e: Endpoint) => get<{ workspace: Workspace }>(e, "/api/state").then((s) => setWs(s.workspace), (x) => setError(String(x)));
  useEffect(() => {
    if (!project) return;
    useStore
      .getState()
      .ensure(project)
      .then((e) => {
        setEp(e);
        void read(e);
      }, (x) => setError(String(x)));
  }, [project]);

  /// A CLI action that streams its output, shown below until it ends.
  function stream(what: string, path: string, params: Record<string, string | boolean | undefined>) {
    if (!ep) return;
    start(
      runKey,
      url(ep, path, params),
      ep.token,
      { what, log: [] as string[] },
      (s, e) => (e.event === "line" || e.event === "fatal" ? { ...s, log: [...s.log, String(e.data)].slice(-200) } : s),
      (s) => s,
    );
  }
  // What the install changed shows once it ends, whether this page was open for it or not.
  const ended = run && !run.running;
  useEffect(() => {
    if (ended && ep) void read(ep);
  }, [ended, ep]);
  async function call(run: () => Promise<unknown>) {
    setError(undefined);
    try {
      await run();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    if (ep) void read(ep);
  }

  const close = () => useStore.setState({ extensions: false });
  const counts: Record<Section, number> = {
    skills: ws?.skills.length ?? 0,
    agents: ws?.agents.length ?? 0,
    mcp: ws?.mcp_servers.length ?? 0,
    plugins: ws?.plugins.length ?? 0,
    hooks: ws?.hooks.length ?? 0,
    commands: ws?.commands.length ?? 0,
  };
  const labels: Record<Section, string> = { skills: "Skills", agents: "Subagents", mcp: "MCP servers", plugins: "Plugins", hooks: "Hooks", commands: "Commands" };
  return (
    <div className="settings">
      <header className="lane-header">
        <div className="lane-heading">
          <span className="lane-name">Extensions</span>
          <span className="muted small">{name ? `what Claude Code loads in ${name}` : "Open a project first"}</span>
        </div>
        <button onClick={close}>Done</button>
      </header>
      <Tabs
        tabs={(Object.keys(labels) as Section[]).map((id) => ({
          id,
          label: (
            <>
              {labels[id]} <Count n={counts[id]} />
            </>
          ),
        }))}
        value={section}
        onChange={setSection}
      />
      <div className="settings-body" style={{ maxWidth: 860 }}>
        {error && <div className="error" style={{ margin: "8px 0" }}>{error}</div>}
        {!ws || !ep ? (
          <p className="muted">Reading the project…</p>
        ) : section === "skills" ? (
          <Skills ep={ep} ws={ws} call={call} />
        ) : section === "agents" ? (
          <Agents ep={ep} ws={ws} call={call} />
        ) : section === "mcp" ? (
          <McpServers ws={ws} stream={stream} running={running} />
        ) : section === "plugins" ? (
          <Plugins ep={ep} ws={ws} stream={stream} running={running} />
        ) : section === "hooks" ? (
          <Hooks ws={ws} />
        ) : (
          <Commands ep={ep} ws={ws} call={call} />
        )}
        {(running || log.length > 0) && (
          <section className="settings-section">
            <h3>{running ?? "Done"}</h3>
            <pre className="code">{log.join("\n") || "…"}</pre>
          </section>
        )}
      </div>
    </div>
  );
}

const scopeLabel = (s: string) => ({ project: "this project", user: "yours", personal: "yours", plugin: "plugin", local: "this machine" })[s] ?? s;

function List({ items, empty, remove }: { items: Named[]; empty: string; remove?: (n: Named) => void }) {
  if (!items.length) return <p className="muted">{empty}</p>;
  return (
    <ul className="rules">
      {items.map((n) => (
        <li key={`${n.scope}:${n.name}:${n.path ?? ""}`} style={{ alignItems: "flex-start", padding: "6px 0", borderBottom: "1px solid var(--line)" }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontWeight: 500 }}>
              {n.name}{" "}
              <span className="pill" style={{ marginLeft: 6 }}>
                {n.plugin ? `plugin · ${n.plugin}` : scopeLabel(n.scope)}
              </span>
              {n.generated && <span className="pill" style={{ marginLeft: 4 }}>generated</span>}
            </div>
            {n.description && <div className="small muted">{n.description}</div>}
          </div>
          {remove && n.scope === "project" && !n.plugin && (
            <button className="link small" onClick={() => remove(n)}>
              Remove
            </button>
          )}
        </li>
      ))}
    </ul>
  );
}

function Skills({ ep, ws, call }: { ep: Endpoint; ws: Workspace; call: (run: () => Promise<unknown>) => Promise<void> }) {
  const [catalog, setCatalog] = useState<CatalogSkill[]>();
  const [form, setForm] = useState({ name: "", description: "", instructions: "" });
  useEffect(() => {
    get<{ entries: CatalogSkill[] }>(ep, "/api/skills/catalog").then((c) => setCatalog(c.entries), () => setCatalog([]));
  }, [ep]);
  const dirOf = (path?: string) => (path ? path.replace(/\/SKILL\.md$/i, "") : "");
  return (
    <>
      <section className="settings-section">
        <h2>Installed</h2>
        <List items={ws.skills} empty="No skills yet. Add one from the catalog below, or write your own." remove={(n) => call(() => post(ep, "/api/skills/remove", { dir: dirOf(n.path), commit: true }))} />
      </section>
      <section className="settings-section">
        <h2>From Keel's catalog</h2>
        {!catalog ? (
          <p className="muted">Reading…</p>
        ) : (
          <ul className="rules">
            {catalog.map((c) => (
              <li key={c.id} style={{ alignItems: "flex-start", padding: "6px 0", borderBottom: "1px solid var(--line)" }}>
                <div style={{ flex: 1 }}>
                  <div style={{ fontWeight: 500 }}>{c.id}</div>
                  <div className="small muted">{c.description}</div>
                </div>
                {c.present ? (
                  <span className="small faint">installed</span>
                ) : (
                  <button onClick={() => call(() => post(ep, "/api/skills/add", { id: c.id, commit: true }))}>Add</button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>
      <section className="settings-section">
        <h2>Write a skill</h2>
        <label className="field">
          Name
          <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="release-notes" />
        </label>
        <label className="field" style={{ maxWidth: 560 }}>
          When to use it
          <input value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} placeholder="Use when writing release notes from the git log" />
        </label>
        <label className="field" style={{ maxWidth: 560 }}>
          Instructions
          <textarea rows={6} value={form.instructions} onChange={(e) => setForm({ ...form, instructions: e.target.value })} style={{ font: "inherit", color: "inherit", background: "var(--raised)", border: "1px solid var(--line)", borderRadius: 6, padding: 8 }} />
        </label>
        <button
          className="primary"
          disabled={!form.name.trim() || !form.description.trim()}
          onClick={() => call(async () => {
            await post(ep, "/api/skills/create", { ...form, commit: true });
            setForm({ name: "", description: "", instructions: "" });
          })}
        >
          Create skill
        </button>
      </section>
    </>
  );
}

function Agents({ ep, ws, call }: { ep: Endpoint; ws: Workspace; call: (run: () => Promise<unknown>) => Promise<void> }) {
  return (
    <>
      <section className="settings-section">
        <h2>Subagents</h2>
        <List items={ws.agents} empty="No subagents. Claude Code hands a task to one when its description matches." />
      </section>
      <Create ep={ep} call={call} kind={AGENT} />
    </>
  );
}

function Commands({ ep, ws, call }: { ep: Endpoint; ws: Workspace; call: (run: () => Promise<unknown>) => Promise<void> }) {
  return (
    <>
      <section className="settings-section">
        <h2>Slash commands</h2>
        <List items={ws.commands} empty="No slash commands yet. A command is a saved prompt: type /name in Claude Code and its text is sent." />
      </section>
      <Create ep={ep} call={call} kind={COMMAND} />
    </>
  );
}

/// The two Markdown-with-frontmatter files Keel writes, and the words each form needs.
interface Kind {
  title: string;
  path: string;
  button: string;
  name: string;
  description: [label: string, placeholder: string];
  tools: [label: string, placeholder: string];
  prompt: [label: string, placeholder: string];
}
const AGENT: Kind = {
  title: "Add a subagent",
  path: "/api/agents/create",
  button: "Create subagent",
  name: "reviewer",
  description: ["When to use it", "Use after a change, to review the diff"],
  tools: ["Tools (empty for all)", "Read, Grep, Glob, Bash"],
  prompt: ["Its instructions", "You review diffs for bugs. Read the change, then…"],
};
const COMMAND: Kind = {
  title: "Add a slash command",
  path: "/api/commands/create",
  button: "Create command",
  name: "ship",
  description: ["What it does — shown in the / menu", "Run the checks, commit and open a PR"],
  tools: ["Tools it may use without asking (optional)", "Bash(git push:*), Bash(gh pr create:*)"],
  prompt: ["The prompt it sends ($ARGUMENTS is what you type after it)", "Run make check. If it passes, commit with a message describing $ARGUMENTS and open a PR."],
};

function Create({ ep, call, kind }: { ep: Endpoint; call: (run: () => Promise<unknown>) => Promise<void>; kind: Kind }) {
  const blank = { name: "", description: "", tools: "", prompt: "", scope: "project" };
  const [form, setForm] = useState(blank);
  const set = (k: keyof typeof blank) => (e: { target: { value: string } }) => setForm({ ...form, [k]: e.target.value });
  return (
    <section className="settings-section">
      <h2>{kind.title}</h2>
      <div className="form">
        <label className="field">
          Name
          <input value={form.name} onChange={set("name")} placeholder={kind.name} spellCheck={false} />
        </label>
        <label className="field">
          {kind.description[0]}
          <input value={form.description} onChange={set("description")} placeholder={kind.description[1]} />
        </label>
        <label className="field">
          {kind.tools[0]}
          <input value={form.tools} onChange={set("tools")} placeholder={kind.tools[1]} spellCheck={false} />
        </label>
        <label className="field">
          {kind.prompt[0]}
          <textarea rows={6} value={form.prompt} onChange={set("prompt")} placeholder={kind.prompt[1]} />
        </label>
        <div className="form-row">
          <div className="segmented" role="radiogroup" aria-label="Where it lives">
            {[
              ["project", "This project", "In .claude/ — committed, so everyone who works here gets it"],
              ["user", "Just me", "In ~/.claude/ — every project on this machine, nobody else"],
            ].map(([v, l, t]) => (
              <button key={v} role="radio" aria-checked={form.scope === v} title={t} onClick={() => setForm({ ...form, scope: v })}>
                {l}
              </button>
            ))}
          </div>
          <span className="spacer" />
          <button
            className="primary"
            disabled={!form.name.trim() || !form.description.trim()}
            onClick={() =>
              call(async () => {
                await post(ep, kind.path, { ...form, name: form.name.trim(), commit: true });
                setForm(blank);
              })
            }
          >
            {kind.button}
          </button>
        </div>
      </div>
    </section>
  );
}

function McpServers({ ws, stream, running }: { ws: Workspace; stream: (what: string, path: string, p: Record<string, string | boolean | undefined>) => void; running?: string }) {
  const [form, setForm] = useState({ name: "", transport: "http", target: "", scope: "project" });
  return (
    <>
      <section className="settings-section">
        <h2>MCP servers</h2>
        {!ws.mcp_servers.length ? (
          <p className="muted">No MCP servers. Add one below.</p>
        ) : (
          <ul className="rules">
            {ws.mcp_servers.map((m) => (
              <li key={`${m.scope}:${m.name}`} style={{ padding: "6px 0", borderBottom: "1px solid var(--line)" }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontWeight: 500 }}>
                    {m.name} <span className="pill" style={{ marginLeft: 6 }}>{scopeLabel(m.scope)}</span>
                  </div>
                  {m.endpoint && <div className="small mono muted">{m.endpoint}</div>}
                </div>
                <button className="link small" disabled={!!running} onClick={() => stream(`Removing ${m.name}`, "/api/mcp/remove", { name: m.name, scope: m.scope, commit: true })}>
                  Remove
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section className="settings-section">
        <h2>Add a server</h2>
        <label className="field">
          Name
          <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="linear" />
        </label>
        <div className="actions">
          {(["http", "sse", "stdio"] as const).map((t) => (
            <button key={t} className={form.transport === t ? "primary" : undefined} aria-pressed={form.transport === t} onClick={() => setForm({ ...form, transport: t })}>
              {t}
            </button>
          ))}
        </div>
        <label className="field" style={{ maxWidth: 560 }}>
          {form.transport === "stdio" ? "Command" : "URL"}
          <input value={form.target} onChange={(e) => setForm({ ...form, target: e.target.value })} placeholder={form.transport === "stdio" ? "npx -y @modelcontextprotocol/server-github" : "https://mcp.linear.app/mcp"} />
        </label>
        <div className="actions">
          {(["project", "user", "local"] as const).map((s) => (
            <button key={s} className={form.scope === s ? "primary" : undefined} aria-pressed={form.scope === s} onClick={() => setForm({ ...form, scope: s })}>
              {s === "project" ? "This project (.mcp.json)" : s === "user" ? "All my projects" : "This machine only"}
            </button>
          ))}
        </div>
        <button style={{ marginTop: 10 }} className="primary" disabled={!form.name.trim() || !form.target.trim() || !!running} onClick={() => stream(`Adding ${form.name}`, "/api/mcp/add", { ...form, commit: form.scope === "project" })}>
          Add server
        </button>
      </section>
    </>
  );
}

function Plugins({ ep, ws, stream, running }: { ep: Endpoint; ws: Workspace; stream: (what: string, path: string, p: Record<string, string | boolean | undefined>) => void; running?: string }) {
  const [catalog, setCatalog] = useState<{ suggested: CatalogPlugin[]; all: CatalogPlugin[] }>();
  const [filter, setFilter] = useState("");
  useEffect(() => {
    get<{ suggested: CatalogPlugin[]; all: CatalogPlugin[] }>(ep, "/api/plugins").then(setCatalog, () => setCatalog({ suggested: [], all: [] }));
  }, [ep, ws]);
  const act = (p: InstalledPlugin, action: string) => stream(`${action[0].toUpperCase()}${action.slice(1)} ${p.name}`, "/api/plugins/action", { action, name: p.name, marketplace: p.marketplace ?? undefined, scope: p.scope ?? undefined });
  const shown = (catalog?.all ?? []).filter((p) => !p.installed && `${p.name} ${p.description}`.toLowerCase().includes(filter.toLowerCase())).slice(0, 50);
  return (
    <>
      <section className="settings-section">
        <h2>Installed</h2>
        {!ws.plugins.length ? (
          <p className="muted">No plugins installed.</p>
        ) : (
          <ul className="rules">
            {ws.plugins.map((p) => (
              <li key={`${p.marketplace}:${p.name}`} style={{ padding: "6px 0", borderBottom: "1px solid var(--line)" }}>
                <div style={{ flex: 1 }}>
                  <span style={{ fontWeight: 500 }}>{p.name}</span>{" "}
                  <span className="small faint">
                    {p.marketplace}
                    {p.version ? ` · ${p.version}` : ""}
                    {p.enabled ? "" : " · disabled"}
                  </span>
                </div>
                <button className="link small" disabled={!!running} onClick={() => act(p, p.enabled ? "disable" : "enable")}>
                  {p.enabled ? "Disable" : "Enable"}
                </button>
                <button className="link small" disabled={!!running} onClick={() => act(p, "update")}>
                  Update
                </button>
                <button className="link small" disabled={!!running} onClick={() => act(p, "uninstall")}>
                  Uninstall
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      {catalog && catalog.suggested.filter((p) => !p.installed).length > 0 && (
        <section className="settings-section">
          <h2>Recommended for this repository</h2>
          <Catalog list={catalog.suggested.filter((p) => !p.installed)} running={running} install={(p) => stream(`Installing ${p.name}`, "/api/plugins/install", { name: p.name, marketplace: p.marketplace, commit: true })} />
        </section>
      )}
      <section className="settings-section">
        <h2>All plugins</h2>
        <input className="field" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Filter" aria-label="Filter plugins" />
        {!catalog ? <p className="muted">Reading the marketplaces…</p> : <Catalog list={shown} running={running} install={(p) => stream(`Installing ${p.name}`, "/api/plugins/install", { name: p.name, marketplace: p.marketplace, commit: true })} />}
      </section>
    </>
  );
}

function Catalog({ list, install, running }: { list: CatalogPlugin[]; install: (p: CatalogPlugin) => void; running?: string }) {
  if (!list.length) return <p className="muted">Nothing to show.</p>;
  return (
    <ul className="rules">
      {list.map((p) => (
        <li key={`${p.marketplace}:${p.name}`} style={{ alignItems: "flex-start", padding: "6px 0", borderBottom: "1px solid var(--line)" }}>
          <div style={{ flex: 1 }}>
            <div style={{ fontWeight: 500 }}>
              {p.name} <span className="small faint">{p.marketplace}</span>
            </div>
            <div className="small muted">{p.reason ?? p.description}</div>
          </div>
          <button disabled={!!running} onClick={() => install(p)}>
            Install
          </button>
        </li>
      ))}
    </ul>
  );
}

function Hooks({ ws }: { ws: Workspace }) {
  const fromRepo = ws.hooks.filter((h) => h.scope === "project");
  const files = [...new Set(ws.hooks.map((h) => h.source))];
  return (
    <>
      <p className="small muted hooks-why">
        A hook is a shell command Claude Code runs on your machine when something happens, so Keel lists them and does not write them. Add or edit one in your settings file, or with <code>/hooks</code> in Claude Code.
        {files.length > 0 && (
          <span className="hooks-files">
            {files.map((f) => (
              <button key={f} className="link" onClick={() => void revealItemInDir(f)}>
                {tilde(f)}
              </button>
            ))}
          </span>
        )}
      </p>
      {!ws.hooks.length && <p className="muted">No hooks. Keel runs its own approval hook beside them; that one is not listed.</p>}
      {fromRepo.length > 0 && (
        <p className="small" style={{ color: "var(--warn)", display: "flex", gap: 6, alignItems: "center" }}>
          <Icon name="shield" size={14} /> Hooks from this repository's .claude/settings.json are quarantined before Keel starts an agent, because they are commands anyone who commits here can make your machine run.
        </p>
      )}
      <ul className="hook-list">
        {groupHooks(ws.hooks).map((g) => (
          <li key={g.command + g.source}>
            <div className="hook-head">
              <span className="hook-runs">{g.runs}</span>
              {g.owner && <span className="faint small">from {g.owner}</span>}
              <span className="pill">{scopeLabel(g.scope)}</span>
            </div>
            <div className="hook-events">
              {g.events.map((e) => (
                <span key={e} className="hook-event" title={e}>
                  {EVENTS[e] ?? e}
                </span>
              ))}
            </div>
            <details className="hook-command">
              <summary className="small faint">Show the command · {tilde(g.source)}</summary>
              <pre className="code">{g.command}</pre>
            </details>
          </li>
        ))}
      </ul>
    </>
  );
}

