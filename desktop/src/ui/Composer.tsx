import { useEffect, useRef, useState } from "react";
import { get, url } from "../api";
import { useStore, type Lane } from "../store";
import { ApprovalStrip } from "./Approvals";
import { Icon } from "./icons";

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
  const mode = useStore((s) => s.lanes[lane]?.mode ?? "plan");
  const model = useStore((s) => s.lanes[lane]?.model ?? "");
  const attachments = useStore((s) => s.lanes[lane]?.attachments);
  const queued = useStore((s) => s.lanes[lane]?.queued);
  const models = useStore((s) => s.lanes[lane]?.models);
  const commands = useStore((s) => s.lanes[lane]?.commands);
  const receipt = useStore((s) => s.lanes[lane]?.receipt);
  const [uploading, setUploading] = useState(false);
  const [files, setFiles] = useState<string[]>([]);
  const [choice, setChoice] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const input = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  const write = (text: string) => useStore.getState().draft(lane, text);
  const token = /(?:^|\s)([@/][^\s]*)$/.exec(draft)?.[1];
  const candidates = suggestions(token, files, commands, dismissed);
  useEffect(() => { setChoice(0); setDismissed(false); }, [token]);
  useEffect(() => {
    const el = input.current;
    if (el) { el.style.height = "auto"; el.style.height = `${Math.min(220, Math.max(66, el.scrollHeight))}px`; }
  }, [draft]);
  useEffect(() => {
    const l = useStore.getState().lanes[lane];
    if (!l) return;
    let cancelled = false;
    type Node = { path: string; dir: boolean; children?: Node[] };
    const walk = (nodes: Node[]): string[] => nodes.flatMap((n) => n.dir ? walk(n.children ?? []) : [n.path]);
    void useStore.getState().ensure(l.project).then((ep) => get<Node[]>(ep, "/api/tree", { wt: l.wt })).then((nodes) => { if (!cancelled) setFiles(walk(nodes)); }, () => {});
    return () => { cancelled = true; };
  }, [lane]);
  const unavailable = uploading || submitting || disconnected || elsewhere;
  const hasMessage = Boolean(draft.trim() || attachments?.length);
  function choose(value: string) { write(draft.slice(0, draft.length - (token?.length ?? 0)) + value + " "); input.current?.focus(); }
  function send() {
    if (!hasMessage || unavailable) return;
    const command = draft.trim();
    if (command === "/settings" || command === "/extensions") {
      useStore.setState(command === "/settings" ? { settings: true } : { extensions: true });
      write(""); return;
    }
    if (command === "/clear") {
      const l = useStore.getState().lanes[lane];
      const next = useStore.getState().newLane(l.project, l.isolated, l.agent);
      useStore.getState().select(next); return;
    }
    void useStore.getState().send(lane, draft.trim() ? draft : "Please review the attached files.", attachments);
  }
  async function upload(picked: File[]) {
    const l = useStore.getState().lanes[lane];
    if (!l || !picked.length) return;
    setUploading(true);
    try {
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
        write(useStore.getState().lanes[lane]?.draft ?? "");
      }
    } catch (error) { patchLane(lane, { error: `Could not attach file: ${error}` }); }
    finally { setUploading(false); }
  }
  return <div className="chat-dock">
    <ApprovalStrip lane={lane} />
    {!!queued?.length && <div className="chat-queue" aria-label="Queued messages">{queued.map((message, index) => <div key={message.id}><span className="small faint">Queued</span><span>{message.text}</span><button onClick={() => { useStore.getState().editQueued(lane, message.id); input.current?.focus(); }}>Edit</button><button aria-label={`Remove queued message ${index + 1}`} onClick={() => useStore.getState().removeQueued(lane, message.id)}>×</button></div>)}</div>}
    <ComposerStatus lane={lane} disconnected={disconnected} elsewhere={elsewhere} submitting={submitting} uploading={uploading} busy={busy} mode={mode} />
    {receipt && !submitting && <div className="chat-retry">Submission not confirmed. <button onClick={() => void useStore.getState().send(lane, receipt.prompt)}>Retry same message</button></div>}
    <div className="chat-composer">
      {!!candidates.length && <div id="composer-options" className="composer-suggestions" role="listbox" aria-label="Suggestions">{candidates.map((c, i) => <button id={`suggestion-${i}`} key={c} role="option" aria-selected={choice === i} onMouseDown={(e) => e.preventDefault()} onClick={() => choose(c)}>{c}</button>)}</div>}
      <Attachments lane={lane} />
      <textarea ref={input} value={draft} aria-label="Message the agent" placeholder="Ask, build, or explain…" spellCheck={false}
        aria-controls={candidates.length ? "composer-options" : undefined} aria-activedescendant={candidates.length ? `suggestion-${choice}` : undefined}
        onChange={(e) => write(e.target.value)}
        onPaste={(e) => { const images = Array.from(e.clipboardData.files); if (images.length) { e.preventDefault(); void upload(images); } }}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing || e.keyCode === 229) return;
          if (candidates.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) { e.preventDefault(); setChoice((i) => (i + (e.key === "ArrowDown" ? 1 : -1) + candidates.length) % candidates.length); return; }
          if (e.key === "Escape" && candidates.length) { e.preventDefault(); setDismissed(true); return; }
          if (candidates.length && (e.key === "Tab" || e.key === "Enter") && !e.shiftKey) { e.preventDefault(); choose(candidates[choice]); return; }
          if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); }
        }} />
      <div className="composer-tools"><input ref={picker} type="file" multiple hidden onChange={(e) => { void upload(Array.from(e.target.files ?? [])); e.target.value = ""; }} />
        <button className="ghost composer-attach" disabled={uploading} onClick={() => picker.current?.click()} aria-label="Attach files"><Icon name="plus" size={16} /></button>
        <select aria-label="Execution mode" value={mode} disabled={busy || submitting} onChange={(e) => patchLane(lane, { mode: e.target.value as Lane["mode"] })}><option value="plan">Plan</option><option value="acceptEdits">Build</option></select>
        <input list="chat-models" className="composer-model" aria-label="Model override" placeholder="Default model" value={model} disabled={busy || submitting} onChange={(e) => patchLane(lane, { model: e.target.value })} />
        <datalist id="chat-models">{models?.map((m) => <option key={m.value ?? m.id} value={m.value ?? m.id}>{m.displayName ?? m.display_name ?? m.name}</option>)}</datalist>
        <span className="spacer" /><button className="primary composer-send" disabled={!hasMessage || unavailable} onClick={send}>{busy ? "Queue" : "Send"}<span aria-hidden="true">↑</span></button>
      </div>
    </div>
    <div className="composer-hint">Enter to {busy ? "queue" : "send"} · Shift+Enter for a new line</div>
  </div>;
}

function suggestions(token: string | undefined, files: string[], commands: string[] | undefined, dismissed: boolean) {
  if (!token || dismissed) return [];
  const all = token[0] === "@" ? files.map((f) => `@${f}`) : [...new Set(["/settings", "/extensions", "/clear", ...(commands ?? []).map((c) => `/${c}`)])];
  return all.filter((c) => c.toLowerCase().includes(token.toLowerCase())).slice(0, 8);
}

function Attachments({ lane }: { lane: string }) {
  const attachments = useStore((s) => s.lanes[lane]?.attachments);
  if (!attachments?.length) return null;
  return <div className="composer-attachments">{attachments.map((a, i) => <div key={`${a.path}:${i}`}>
    {a.preview && <img src={a.preview} alt={a.name} />}<span>{a.name}</span>
    <button aria-label={`Remove ${a.name}`} onClick={() => { if (a.preview) URL.revokeObjectURL(a.preview); patchLane(lane, { attachments: attachments.filter((_, at) => at !== i) }); }}>×</button>
  </div>)}</div>;
}

function ComposerStatus({ lane, disconnected, elsewhere, submitting, uploading, busy, mode }: {
  lane: string; disconnected: boolean; elsewhere: boolean; submitting: boolean; uploading: boolean; busy: boolean; mode: Lane["mode"];
}) {
  const text = disconnected ? "Reconnecting — your draft is saved" : elsewhere ? "Following an external session — stop it there before resuming here" : submitting ? "Sending…" : uploading ? "Attaching…" : busy ? "Working in this lane" : mode === "plan" ? "Plan · Explore before making changes" : "Build · Changes stay in this lane’s checkout";
  return <div className="chat-status" role="status">{text}{busy && <button onClick={() => void useStore.getState().stop(lane)}>Stop</button>}</div>;
}
