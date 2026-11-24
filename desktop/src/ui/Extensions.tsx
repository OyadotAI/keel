import { UpdateButton } from "./UpdateButton";
import { missing, SKILLS_CHANGED, useSkillCatalog } from "../recommended-skills";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { EVENTS, groupHooks, tilde } from "../hooks";
import { useEffect, useRef, useState } from "react";
import { get, post, url, type Endpoint } from "../api";
import { start, useRun } from "../runs";
import { TeamUpdate, useTeamPlan } from "./TeamUpdate";
import { absent, forAgents, others } from "../team";
import { recordShown } from "../setup";
import { useStore } from "../store";
import { Icon } from "./icons";
import { MemberAvatar } from "./MemberAvatar";
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
interface AgentProfile { name: string; display_name: string; avatar?: string | null }
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
  const requestedSection = useStore((s) => s.extensionsSection);
  useEffect(() => {
    if (requestedSection) {
      setSection(requestedSection);
      useStore.setState({ extensionsSection: undefined });
    }
  }, [requestedSection]);
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
        <UpdateButton />
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
          <Agents ep={ep} ws={ws} call={call} project={project ?? ""} />
        ) : section === "mcp" ? (
          <McpServers ep={ep} ws={ws} stream={stream} running={running} />
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
            {n.description && <div className="small muted ext-desc" title={n.description}>{n.description}</div>}
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

const SKILL_GROUPS: [string, (n: Named) => boolean][] = [
  ["This project", (n) => !n.plugin && n.scope === "project"],
  ["Yours", (n) => !n.plugin && n.scope !== "project"],
  ["From plugins", (n) => !!n.plugin],
];

/// Skills, findable: a filter and New skill on top, the list grouped by where each comes from (so
/// no row repeats it), one line of description each, and the catalog one click away. Writing a
/// skill used to sit below all seventy of them.
function Skills({ ep, ws, call }: { ep: Endpoint; ws: Workspace; call: (run: () => Promise<unknown>) => Promise<void> }) {
  const catalog = useSkillCatalog();
  const [writing, setWriting] = useState(false);
  const [filter, setFilter] = useState("");
  const add = (id: string) => call(async () => { await post(ep, "/api/skills/add", { id, commit: true }); window.dispatchEvent(new Event(SKILLS_CHANGED)); });
  const recommended = missing(catalog ?? []);
  const dirOf = (path?: string) => (path ? path.replace(/\/SKILL\.md$/i, "") : "");
  const q = filter.trim().toLowerCase();
  const shown = ws.skills.filter((n) => !q || `${n.name} ${n.description ?? ""}`.toLowerCase().includes(q));
  const remove = (n: Named) => call(async () => { await post(ep, "/api/skills/remove", { dir: dirOf(n.path), commit: true }); window.dispatchEvent(new Event(SKILLS_CHANGED)); });
  return (
    <section className="settings-section skills">
      <div className="skills-bar">
        <input className="field" aria-label="Filter skills" placeholder={`Filter ${ws.skills.length} skills`} value={filter} onChange={(e) => setFilter(e.target.value)} />
        <button onClick={() => setWriting(true)}><Icon name="plus" size={13} />New skill</button>
      </div>
      {writing && <WriteSkill ep={ep} call={call} close={() => setWriting(false)} />}
      {recommended.length > 0 && <div className="skill-recommended"><h3>Recommended for this project <span className="tab-count">{recommended.length}</span></h3>
        <ul className="skill-list">{recommended.map((c) => <li key={c.id} title={c.description}>
          <span className="skill-name">{c.id}</span><span className="skill-desc">{c.recommended} {c.description}</span>
          <button className="primary" onClick={() => add(c.id)}>Add</button>
        </li>)}</ul></div>}
      {!ws.skills.length && <p className="muted">No skills yet. Write one, or add one from the catalog below.</p>}
      {ws.skills.length > 0 && !shown.length && <p className="muted">No skill matches “{filter}”.</p>}
      {SKILL_GROUPS.map(([title, keep]) => {
        const group = shown.filter(keep);
        return group.length > 0 && <div key={title} className="skill-group"><h3>{title} <span className="tab-count">{group.length}</span></h3>
          <ul className="skill-list">{group.map((n) => <li key={`${n.scope}:${n.name}:${n.path ?? ""}`} title={n.description ?? undefined}>
            <span className="skill-name">{n.name}</span>
            <span className="skill-desc">{n.plugin ? `${n.plugin} · ` : ""}{n.description}</span>
            {n.scope === "project" && !n.plugin && <span className="ext-edit"><button className="link small" onClick={() => remove(n)}>Remove</button></span>}
          </li>)}</ul></div>;
      })}
      <details className="delivery-section skill-catalog"><summary>Add from Oya Factory's catalog</summary>
        {!catalog ? <p className="muted">Reading…</p> : <ul className="skill-list">{catalog.map((c) => <li key={c.id} title={c.description}>
          <span className="skill-name">{c.id}</span><span className="skill-desc">{c.description}</span>
          {c.present ? <span className="small faint">Added</span> : <button className="link small" onClick={() => add(c.id)}>Add</button>}
        </li>)}</ul>}
      </details>
    </section>
  );
}

/// The write-a-skill form, opened from New skill at the top of the list.
function WriteSkill({ ep, call, close }: { ep: Endpoint; call: (run: () => Promise<unknown>) => Promise<void>; close: () => void }) {
  const [form, setForm] = useState({ name: "", description: "", instructions: "" });
  return <form className="skill-form" onSubmit={(e) => { e.preventDefault(); void call(async () => { await post(ep, "/api/skills/create", { ...form, commit: true }); close(); }); }}>
    <label>Name<input className="field" autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="release-notes" /></label>
    <label>When to use it<input className="field" value={form.description} onChange={(e) => setForm({ ...form, description: e.target.value })} placeholder="Use when writing release notes from the git log" /></label>
    <label>Instructions<textarea className="field" rows={5} value={form.instructions} onChange={(e) => setForm({ ...form, instructions: e.target.value })} /></label>
    <div className="schedule-foot"><p className="small muted">Saved to this project and committed.</p><button type="button" className="link" onClick={close}>Cancel</button><button type="submit" className="primary" disabled={!form.name.trim() || !form.description.trim()}>Save skill</button></div>
  </form>;
}

function Agents({ ep, ws, call, project }: { ep: Endpoint; ws: Workspace; call: (run: () => Promise<unknown>) => Promise<void>; project: string }) {
  const [plan, reread] = useTeamPlan(ep);
  const [profiles, setProfiles] = useState<AgentProfile[]>([]);
  const [installing, setInstalling] = useState(false);
  const trusted = useStore((s) => !!s.trusted[project]);
  const record = useStore((s) => s.setupRun[project]);
  // In a trusted project Oya Factory adds the team itself; a button stays only for a failed attempt.
  const auto = trusted && recordShown(record) ? record : undefined;
  const autoFailed = auto?.phase === "failed" && !!auto.error;
  const retry = () => void post(ep, "/api/setup/run").catch(() => undefined);
  const missing = ["pm", "designer", "principal", "qa", "reviewer", "security", "reliability", "oya-browser"].some((name) => !ws.agents.some((agent) => agent.name === name));
  function installTeam() {
    setInstalling(true);
    void call(async () => {
      await post(ep, "/api/adopt", undefined, { commit: true });
      window.dispatchEvent(new Event("keel:subagents-changed"));
    }).finally(() => setInstalling(false));
  }
  useEffect(() => { get<AgentProfile[]>(ep, "/api/agents/profiles").then(setProfiles, () => setProfiles([])); }, [ep, ws.agents.length]);
  async function save(profile: AgentProfile) {
    await call(() => post(ep, "/api/agents/profiles", profile));
    const next = await get<AgentProfile[]>(ep, "/api/agents/profiles");
    setProfiles(next);
    window.dispatchEvent(new Event("keel:subagents-changed"));
  }
  return (
    <>
      <section className="settings-section">
        <h2>Subagents</h2>
        {/* Above the members: the update is the action, and eight rows pushed it out of view. */}
        <TeamUpdate key={project} ep={ep} project={project} plan={plan} reread={reread} />
        {!ws.agents.length ? <>
          <p className="muted">{trusted ? "Oya Factory adds its default team to this project on its own. Once it is in, members appear in the sidebar and you can tag them with @." : "Install Oya Factory’s default team, or create your own subagent below. Once installed, members appear in the sidebar and you can tag them with @."}</p>
          {trusted ? (auto?.phase === "running" ? <p className="small muted" role="status">Oya Factory is adding its default team to this project…</p> : autoFailed ? <><p className="small error">Couldn’t add the team: {auto!.error}</p><button onClick={retry}>Try adding the team again</button></> : null) : <button disabled={installing} onClick={installTeam}>{installing ? "Installing team…" : "Install default team"}</button>}
          <p className="small muted">Adds eight members for {forAgents(plan)}, including Oya Browser, role playbooks, handoff templates and a production checklist. {absent(plan)}</p>
        </> : <ul className="rules">
          {ws.agents.map((agent) => {
            const profile = profiles.find((p) => p.name === agent.name) ?? { name: agent.name, display_name: "", avatar: null };
            return <AgentProfileRow key={`${agent.scope}:${agent.name}`} agent={agent} profile={profile} save={save} />;
          })}
        </ul>}
        {!!ws.agents.length && missing && <div className="team-install">
          {trusted ? (autoFailed ? <button onClick={retry}>Try again</button> : null) : <button disabled={installing} onClick={installTeam}>{installing ? "Adding members…" : "Add missing team members"}</button>}
          <p className="small muted">{trusted ? `Oya Factory adds the missing members for ${forAgents(plan)} on its own. Keeps your existing definitions and images.` : `Adds what's missing for ${forAgents(plan)}. Keeps your existing definitions and images.`}{trusted && autoFailed ? ` Last attempt: ${auto!.error}` : ""}</p>
        </div>}
      </section>
      <Create ep={ep} call={call} kind={AGENT} />
    </>
  );
}

function AgentProfileRow({ agent, profile, save }: { agent: Named; profile: AgentProfile; save: (p: AgentProfile) => Promise<void> }) {
  const picker = useRef<HTMLInputElement>(null);
  const [displayName, setDisplayName] = useState(profile.display_name);
  useEffect(() => setDisplayName(profile.display_name), [profile.display_name]);
  async function image(file?: File) {
    if (!file) return;
    if (!file.type.startsWith("image/") || file.size > 1_000_000) return;
    const avatar = await new Promise<string>((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(reader.error); reader.readAsDataURL(file); });
    await save({ ...profile, display_name: displayName, avatar });
  }
  return <li style={{ gap: 12, alignItems: "center", padding: "10px 0", borderBottom: "1px solid var(--line)" }}>
    <button className="ghost" aria-label={`Choose ${agent.name} avatar`} title="Choose avatar image" onClick={() => picker.current?.click()} style={{ padding: 0, borderRadius: 8, overflow: "hidden", width: 42, height: 42, flex: "none" }}>
      <MemberAvatar name={agent.name} displayName={displayName} avatar={profile.avatar} size={42} />
    </button>
    <input ref={picker} type="file" accept="image/png,image/jpeg,image/webp,image/gif" hidden onChange={(e) => { void image(e.target.files?.[0]); e.target.value = ""; }} />
    <div style={{ flex: 1, minWidth: 130 }}><div style={{ fontWeight: 500 }}>{agent.name}<span className="pill" style={{ marginLeft: 6 }}>{scopeLabel(agent.scope)}</span></div><div className="small muted">@{agent.name} in chat</div></div>
    <span className="ext-edit"><input aria-label={`${agent.name} display name`} value={displayName} placeholder="Display name" onChange={(e) => setDisplayName(e.target.value)} onBlur={() => { if (displayName !== profile.display_name) void save({ ...profile, display_name: displayName }); }} />
    <button className="link small" onClick={() => picker.current?.click()}>Image…</button></span>
  </li>;
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

/// The two Markdown-with-frontmatter files Oya Factory writes, and the words each form needs.
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
      {kind === AGENT && <p className="small muted">Subagents you add here are for Claude Code only. Oya Factory's team is installed for every coding agent on this machine.</p>}
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
              ["user", "Just me", "In ~/.claude/ — every project on this machine, nobody else. Claude Code only."],
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
                if (kind === AGENT) window.dispatchEvent(new Event("keel:subagents-changed"));
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

/** Which coding agents a new server reaches, said before the click: only "All my projects" is
 *  copied to Antigravity and Codex, because their own MCP config is machine-wide. */
function ScopeHint({ ep, scope }: { ep: Endpoint; scope: string }) {
  const [plan] = useTeamPlan(ep);
  const to = others(plan);
  if (!to) return null;
  return <p className="small muted">{scope === "user" ? `Also added to ${to}, unless it carries secrets.` : "Claude Code only."}</p>;
}

function McpServers({ ep, ws, stream, running }: { ep: Endpoint; ws: Workspace; stream: (what: string, path: string, p: Record<string, string | boolean | undefined>) => void; running?: string }) {
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
        <ScopeHint ep={ep} scope={form.scope} />
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
                    {[source(p.marketplace), p.version, p.enabled ? "" : "disabled"].filter(Boolean).join(" · ")}
                  </span>
                </div>
                <span className="ext-edit">
                <button className="link small" disabled={!!running} onClick={() => act(p, p.enabled ? "disable" : "enable")}>
                  {p.enabled ? "Disable" : "Enable"}
                </button>
                <button className="link small" disabled={!!running} onClick={() => act(p, "update")}>
                  Update
                </button>
                <button className="link small" disabled={!!running} onClick={() => act(p, "uninstall")}>
                  Uninstall
                </button>
                </span>
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

/// A hook's events as one line: the first three, then how many more (all of them on hover).
function eventLine(events: string[]): string {
  const named = events.map((e) => EVENTS[e] ?? e);
  return named.length <= 3 ? named.join(" · ") : `${named.slice(0, 3).join(" · ")} · +${named.length - 3} more`;
}

/// Where a plugin comes from, when that is worth reading: the official marketplace is every other
/// row, and "marketplace_entry" is a placeholder in the catalog's data rather than a name.
function source(marketplace?: string | null): string {
  return !marketplace || marketplace === "claude-plugins-official" || marketplace === "marketplace_entry" ? "" : marketplace;
}

function Catalog({ list, install, running }: { list: CatalogPlugin[]; install: (p: CatalogPlugin) => void; running?: string }) {
  if (!list.length) return <p className="muted">Nothing to show.</p>;
  return (
    <ul className="rules">
      {list.map((p) => (
        <li key={`${p.marketplace}:${p.name}`} style={{ alignItems: "flex-start", padding: "6px 0", borderBottom: "1px solid var(--line)" }}>
          <div style={{ flex: 1 }}>
            <div style={{ fontWeight: 500 }}>
              {p.name} <span className="small faint">{source(p.marketplace)}</span>
            </div>
            <div className="small muted ext-desc" title={p.reason ?? p.description ?? undefined}>{p.reason ?? p.description}</div>
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
        A hook is a shell command Claude Code runs on your machine when something happens, so Oya Factory lists them and does not write them. Add or edit one in your settings file, or with <code>/hooks</code> in Claude Code.
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
      {!ws.hooks.length && <p className="muted">No hooks. Oya Factory runs its own approval hook beside them; that one is not listed.</p>}
      {fromRepo.length > 0 && (
        <p className="small" style={{ color: "var(--warn)", display: "flex", gap: 6, alignItems: "center" }}>
          <Icon name="shield" size={14} /> Hooks from this repository's .claude/settings.json and .agents/hooks.json are quarantined before Oya Factory starts an agent, because they are commands anyone who commits here can make your machine run.
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
            <p className="hook-events" title={g.events.map((e) => EVENTS[e] ?? e).join(" · ")}>{eventLine(g.events)}</p>
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

