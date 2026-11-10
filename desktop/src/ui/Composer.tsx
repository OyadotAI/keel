// Native chat composition keeps file references, uploads, and queued messages as separate intent.
import { useEffect, useMemo, useRef, useState } from "react";
import { get, url } from "../api";
import { AGENT_NAME, EFFORTS, rememberedCommands, useStore, type Effort, type Lane } from "../store";
import { LaneDock } from "./PreReview";
import { Working } from "./Working";
import { activity } from "./StatusBar";
import { Icon } from "./icons";
import { useFileDrag } from "../file-drag";
import { consumeChatFocus } from "../chat-focus";
import { FileReferences } from "./FileReferences";

export function patchLane(lane: string, patch: Partial<Lane>) {
  useStore.setState((s) => s.lanes[lane] ? { lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], ...patch } } } : s);
  const current = useStore.getState().lanes[lane];
  if (current) useStore.getState().draft(lane, current.draft ?? "");
}

export function Composer({ lane }: { lane: string }) {
  const draft = useStore((s) => s.lanes[lane]?.draft ?? "");
  const busy = useStore((s) => !!s.lanes[lane]?.running);
  const submitting = useStore((s) => !!s.lanes[lane]?.submitting);
  const disconnected = useStore((s) => !!s.lanes[lane]?.disconnected);
  const elsewhere = useStore((s) => !!s.lanes[lane]?.elsewhere);
  const model = useStore((s) => s.lanes[lane]?.model ?? "");
  const attachments = useStore((s) => s.lanes[lane]?.attachments);
  const references = useStore((s) => s.lanes[lane]?.references);
  const checking = useStore((s) => !!s.lanes[lane]?.referencePending);
  const dragOver = useFileDrag((s) => s.drag?.target === lane);
  const queued = useStore((s) => s.lanes[lane]?.queued);
  const models = useStore((s) => s.lanes[lane]?.models);
  const agentId = useStore((s) => s.lanes[lane]?.agent);
  const reported = useStore((s) => s.lanes[lane]?.commands);
  const commands = useMemo(() => reported ?? rememberedCommands(agentId), [reported, agentId]);
  const receipt = useStore((s) => s.lanes[lane]?.receipt);
  // Claude and Codex take words mid-turn; Antigravity's one-shot turn can only queue them.
  const joins = useStore((s) => s.lanes[lane]?.agent !== "antigravity" && !s.lanes[lane]?.queued.length);
  const [uploading, setUploading] = useState(false);
  const [files, setFiles] = useState<string[]>([]);
  const [subagents, setSubagents] = useState<{ name: string; display_name: string }[]>([]);
  const [choice, setChoice] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const input = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { if (input.current) consumeChatFocus(lane, input.current); }, [lane]);
  const picker = useRef<HTMLInputElement>(null);
  const write = (text: string) => useStore.getState().draft(lane, text);
  const refreshSubagents = () => void readSubagents(lane).then(setSubagents, () => undefined);
  const token = /(?:^|\s)([@/][^\s]*)$/.exec(draft)?.[1];
  const candidates = suggestions(token, files, commands, dismissed, subagents);
  useEffect(() => { setChoice(0); setDismissed(false); }, [token]);
  useEffect(() => {
    const el = input.current;
    if (el) { el.style.height = "auto"; el.style.height = `${Math.min(COMPOSER_MAX_PX, Math.max(COMPOSER_MIN_PX, el.scrollHeight))}px`; }
  }, [draft]);
  useEffect(() => {
    const l = useStore.getState().lanes[lane];
    if (!l) return;
    let cancelled = false;
    type Node = { path: string; dir: boolean; children?: Node[] };
    const walk = (nodes: Node[]): string[] => nodes.flatMap((n) => n.dir ? walk(n.children ?? []) : [n.path]);
    void useStore.getState().ensure(l.project).then(async (ep) => {
      const [nodes, agents] = await Promise.all([get<Node[]>(ep, "/api/tree", { wt: l.wt }), get<{ name: string; display_name: string }[]>(ep, "/api/agents/profiles")]);
      if (!cancelled) { setFiles(walk(nodes)); setSubagents(agents); }
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [lane]);
  const unavailable = uploading || checking || submitting || disconnected || elsewhere;
  const hasMessage = Boolean(draft.trim() || attachments?.length || references?.length);
  function choose(value: string) {
    const tagged = value.startsWith("@agent:");
    const insertion = tagged ? `@${value.slice(7)}` : value;
    write(draft.slice(0, draft.length - (token?.length ?? 0)) + insertion + " "); input.current?.focus();
  }
  function send() {
    if (!hasMessage || unavailable) return;
    const command = draft.trim();
    if (command === "/settings" || command === "/extensions") {
      useStore.setState(command === "/settings" ? { settings: true } : { extensions: true });
      write(""); return;
    }
    if (command === "/clear") {
      useStore.getState().newConversationFrom(lane); return;
    }
    const prompt = draft.trim() ? draft : references?.length ? "Please review the referenced files." : "Please review the attached files.";
    const mentions = new Set(Array.from(prompt.matchAll(/(?:^|\s)@([a-z0-9-]+)(?=$|\s|[.,!?])/g), (match) => match[1]));
    const tagged = subagents.filter((agent) => mentions.has(agent.name));
    const system = tagged.length ? `The user explicitly tagged these Claude Code subagents: ${tagged.map((agent) => `\`${agent.name}\``).join(", ")}. Use the Agent tool (called Task in older versions) to delegate the requested work to each tagged subagent, using its exact name as subagent_type. Pass the request and relevant context, then integrate their results. A direct mention does not require unrelated pipeline stages. If a member cannot run, report the actual blocker. The visible user message is the request; the @name tokens identify the subagents.` : undefined;
    void useStore.getState().send(lane, prompt, attachments, system, references);
  }
  async function upload(picked: File[]) {
    setUploading(true);
    try { await uploadFiles(lane, picked); } catch (error) { patchLane(lane, { error: `Could not attach file: ${error}` }); }
    finally { setUploading(false); }
  }
  return <div className="chat-dock" data-reference-target={lane}>
    <LaneDock lane={lane} />
    {!!queued?.length && <div className="chat-queue" aria-label="Queued messages">{queued.map((message, index) => <div key={message.id}><span className="small faint">{message.failed ? "Not sent · edit to retry" : "Queued"}</span><span>{message.text}</span><button onClick={() => { useStore.getState().editQueued(lane, message.id); input.current?.focus(); }}>Edit</button><button aria-label={`Remove queued message ${index + 1}`} onClick={() => useStore.getState().removeQueued(lane, message.id)}>×</button></div>)}</div>}
    <ComposerStatus lane={lane} disconnected={disconnected} elsewhere={elsewhere} submitting={submitting} uploading={uploading} busy={busy} />
    {receipt && !submitting && <div className="chat-retry">Submission not confirmed. <button onClick={() => void useStore.getState().send(lane, receipt.prompt, receipt.attachments, receipt.system, receipt.references)}>Retry same message</button><button onClick={() => useStore.getState().discardReceipt(lane)}>Discard</button></div>}
    <div className={composerClass(dragOver, busy)}>
      {dragOver && <div className="reference-drop-hint">Drop to reference this file.</div>}
      {!!candidates.length && <div id="composer-options" className="composer-suggestions" role="listbox" aria-label="Suggestions">{candidates.map((c, i) => {
        const agent = c.startsWith("@agent:") ? subagents.find((a) => a.name === c.slice(7)) : undefined;
        return <button id={`suggestion-${i}`} key={c} role="option" aria-selected={choice === i} onMouseDown={(e) => e.preventDefault()} onClick={() => choose(c)}>{agent ? `@${agent.display_name || agent.name} · ${agent.name}` : c}</button>;
      })}</div>}
      <Attachments lane={lane} />
      <FileReferences lane={lane} />
      <textarea ref={input} data-chat-input={lane} value={draft} aria-label="Message the agent" placeholder="Ask, build, or explain…" spellCheck={false}
        aria-controls={candidates.length ? "composer-options" : undefined} aria-activedescendant={candidates.length ? `suggestion-${choice}` : undefined}
        onChange={(e) => { write(e.target.value); if (e.target.value.endsWith("@")) void refreshSubagents(); }}
        onPaste={(e) => { const images = Array.from(e.clipboardData.files); if (images.length) { e.preventDefault(); void upload(images); } }}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing || e.keyCode === 229) return;
          if (candidates.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) { e.preventDefault(); setChoice((i) => (i + (e.key === "ArrowDown" ? 1 : -1) + candidates.length) % candidates.length); return; }
          if (e.key === "Escape" && candidates.length) { e.preventDefault(); setDismissed(true); return; }
          if (candidates.length && (e.key === "Tab" || e.key === "Enter") && !e.shiftKey) { e.preventDefault(); choose(candidates[choice]); return; }
          if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); return; }
          if (e.key === "Escape" && busy) { e.preventDefault(); void useStore.getState().stop(lane); return; }
          if (e.key === "ArrowUp" && !draft) { const last = lastPrompt(lane); if (last) { e.preventDefault(); write(last); } }
        }} />
      <div className="composer-tools"><input ref={picker} type="file" multiple hidden onChange={(e) => { void upload(Array.from(e.target.files ?? [])); e.target.value = ""; }} />
        <button className="ghost composer-attach" disabled={uploading} onClick={() => picker.current?.click()} aria-label="Attach files"><Icon name="plus" size={16} /></button>
        <ComposerSettings lane={lane} model={model} models={models} busy={busy} />
        <span className="spacer" />{busy && <button className="composer-stop" onClick={() => void useStore.getState().stop(lane)} title="Stop the turn (Esc)">Stop</button>}<button className="primary composer-send" disabled={!hasMessage || unavailable} onClick={send}>{busy && !joins ? "Queue" : "Send"}<span aria-hidden="true">↑</span></button>
      </div>
    </div>
    {busy && <div className="composer-hint">{joins ? "Enter adds to the running turn" : "Enter to queue"} · Esc to stop</div>}
  </div>;
}

/** The message box grows with what is typed, from one comfortable line to a cap that leaves the
 *  conversation most of a small screen. */
const COMPOSER_MIN_PX = 44, COMPOSER_MAX_PX = 160;

/** The composer's look: a file held over it, or the agent at work (its aura). */
function composerClass(dragOver: boolean, busy: boolean): string {
  return ["chat-composer", dragOver && "reference-over", busy && "is-working"].filter(Boolean).join(" ");
}

/** The last thing typed in this lane, for ↑ in an empty composer as in the CLI. */
function lastPrompt(lane: string): string | undefined {
  const turns = useStore.getState().lanes[lane]?.conv.turns ?? [];
  for (let i = turns.length - 1; i >= 0; i--) { const p = turns[i].prompt; if (p?.trim()) return p; }
}

/** Model and effort behind Options, since the defaults are right for nearly everyone and every
 *  visible control is one more thing to decide before typing. There is no plan/build choice: a
 *  chat lane builds, because people did not know what the modes meant.
 *  All of it can change while a turn runs: the next message takes it, as in the CLI. */
function ComposerSettings({ lane, model, models, busy }: { lane: string; model: string; models: Lane["models"]; busy: boolean }) {
  const effort = useStore((s) => s.lanes[lane]?.effort ?? "");
  const agent = useStore((s) => s.lanes[lane]?.agent);
  const [more, setMore] = useState(!!model || !!effort);
  const next = busy ? " — applies to your next message" : "";
  return <>
    {!more && <button className="ghost composer-more" onClick={() => setMore(true)} title="Choose the model and how hard it thinks">Options</button>}
    {more && <>
      <input list="chat-models" className="composer-model" aria-label="Model" title={`Model${next}`} placeholder="Best model" value={model} onChange={(e) => patchLane(lane, { model: e.target.value })} />
      <datalist id="chat-models">{models?.map((m) => <option key={m.value ?? m.id} value={m.value ?? m.id}>{m.displayName ?? m.display_name ?? m.name}</option>)}</datalist>
      {/* Every agent takes it: Claude's session flag, Codex's turn option, agy's `--effort`. */}
      {<select aria-label="How hard it thinks" title={`How hard it thinks${next}`} value={effort} onChange={(e) => patchLane(lane, { effort: (e.target.value || undefined) as Lane["effort"] })}>
        <option value="">Normal thinking</option>{EFFORTS.filter((level) => agent !== "codex" || level !== "max").map((level) => <option key={level} value={level}>{EFFORT_LABEL[level]}</option>)}
      </select>}
    </>}
  </>;
}
const EFFORT_LABEL: Record<Effort, string> = { low: "Quick", medium: "Balanced", high: "Think hard", xhigh: "Think harder", max: "Think hardest" };

async function readSubagents(lane: string) {
  const current = useStore.getState().lanes[lane];
  if (!current) return [];
  const endpoint = await useStore.getState().ensure(current.project);
  return get<{ name: string; display_name: string }[]>(endpoint, "/api/agents/profiles");
}

function suggestions(token: string | undefined, files: string[], commands: string[] | undefined, dismissed: boolean, subagents: { name: string; display_name: string }[]) {
  if (!token || dismissed) return [];
  if (token[0] === "@") {
    const matches = (value: string) => value.toLowerCase().includes(token.toLowerCase());
    const agents = subagents.map((a) => `@agent:${a.name}`).filter((c) => matches(`@${c.slice(7)}`));
    const paths = files
      .filter((f) => !/(^|\/)\.claude\/agents\/[^/]+\.md$/i.test(f))
      .map((f) => `@${f}`)
      .filter(matches);
    return [...agents, ...paths].slice(0, 8);
  }
  return [...new Set(["/settings", "/extensions", "/clear", ...(commands ?? []).map((c) => `/${c}`)])]
    .filter((c) => c.toLowerCase().includes(token.toLowerCase())).slice(0, 8);
}

function Attachments({ lane }: { lane: string }) {
  const attachments = useStore((s) => s.lanes[lane]?.attachments);
  if (!attachments?.length) return null;
  return <div className="composer-attachments">{attachments.map((a, i) => <div key={`${a.path}:${i}`}>
    {a.preview && <img src={a.preview} alt={a.name} />}<span>{a.name}</span>
    <button aria-label={`Remove ${a.name}`} onClick={() => { if (a.preview) URL.revokeObjectURL(a.preview); patchLane(lane, { attachments: attachments.filter((_, at) => at !== i) }); }}>×</button>
  </div>)}</div>;
}

function ComposerStatus({ lane, disconnected, elsewhere, submitting, uploading, busy }: {
  lane: string; disconnected: boolean; elsewhere: boolean; submitting: boolean; uploading: boolean; busy: boolean;
}) {
  const waiting = useStore((s) => !!s.lanes[lane]?.pending.length);
  const progress = useStore((s) => s.lanes[lane] ? activity(s.lanes[lane]).label : "Working…");
  const agent = useStore((s) => AGENT_NAME[s.lanes[lane]?.agent ?? "claude"]);
  const live = busy && !waiting && !disconnected && !elsewhere;
  const text = disconnected ? "Reconnecting — your draft is saved" : elsewhere ? "Following an external session — stop it there before resuming here" : waiting ? "Waiting for your response" : submitting ? "Sending…" : uploading ? "Attaching…" : busy ? `${agent} · ${progress}` : "";
  if (!text) return null;
  return <div className={`chat-status${live ? " is-working" : ""}`} role="status"><span className="chat-status-text">{(live || submitting || uploading) && <Working />}<span>{text}</span></span></div>;
}

async function uploadFiles(lane: string, picked: File[]) {
  if (!picked.length || !useStore.getState().lanes[lane]) return;
  const ready = await useStore.getState().prepare(lane);
  if (!ready) throw new Error("Could not prepare the checkout");
  for (const file of picked) {
    if (file.size > 10 * 1024 * 1024) throw new Error(`${file.name} exceeds the 10 MB attachment limit.`);
    const response = await fetch(url(ready.ep, "/api/attach", { name: file.name, wt: ready.wt }), {
      method: "POST", headers: { Authorization: `Bearer ${ready.ep.token}`, "Content-Type": "application/octet-stream" }, body: file, signal: AbortSignal.timeout(90_000),
    });
    if (!response.ok) throw new Error(await response.text());
    const attachment = await response.json() as { path: string };
    const current = useStore.getState().lanes[lane]?.attachments ?? [];
    patchLane(lane, { attachments: [...current, { path: attachment.path, name: file.name, mime: file.type, preview: file.type.startsWith("image/") ? URL.createObjectURL(file) : undefined }] });
  }

}
