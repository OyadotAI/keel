// Read-only provider telemetry. Never injects prompts, counts through a paid API, or edits settings.
import { open, readdir, stat } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { homedir } from 'node:os';
import { randomUUID } from 'node:crypto';

const LIMIT = 200;
const number = (n) => Number.isFinite(n) && n >= 0 ? n : undefined;
const array = (v) => Array.isArray(v) ? v : [];

async function markdown(path) {
  const file = await open(path, 'r');
  try {
    const meta = await file.stat();
    if (!meta.isFile() || meta.size > 128 * 1024) return '';
    const buffer = Buffer.alloc(Math.min(meta.size, 128 * 1024));
    const { bytesRead } = await file.read(buffer, 0, buffer.length, 0);
    return buffer.subarray(0, bytesRead).toString('utf8');
  } finally { await file.close(); }
}

export class ContextInspector {
  constructor(provider, cwd, send) {
    this.cwd = cwd; this.send = send; this.calls = new Map(); this.members = new Map();
    this.state = { provider, session: '', updated_at: new Date().toISOString(), origin: 'runtime', categories: [], memory: [], skills: [], evidence: [], limitations: [] };
  }
  update(patch = {}, persist = false) {
    Object.assign(this.state, patch, { updated_at: new Date().toISOString() });
    this.send('context', { snapshot: this.state, persist });
    return this.state;
  }
  limitation(message) {
    if (!this.state.limitations.includes(message)) this.state.limitations.push(message);
  }
  skill(name, patch = {}) {
    if (typeof name !== 'string' || !name || name.length > 160) return;
    if (typeof patch.description === 'string') patch.description = patch.description.slice(0, 2000);
    let skill = this.state.skills.find((s) => s.name === name && (!patch.path || !s.path || s.path === patch.path));
    if (!skill) { skill = { name }; this.state.skills.push(skill); }
    Object.assign(skill, patch);
    this.state.skills = this.state.skills.slice(-LIMIT);
    return skill;
  }
  evidence(e) {
    const key = `${e.id}:${e.kind}:${e.name ?? e.path}`;
    if (this.state.evidence.some((v) => v.key === key)) return;
    this.state.evidence.push({ ...e, key, at: new Date().toISOString(), turn: e.turn ?? this.turn });
    this.state.evidence = this.state.evidence.slice(-LIMIT);
  }
  memory(path, patch = {}) {
    if (typeof path !== 'string' || !path || path.startsWith('<') || /^https?:/.test(path)) return;
    const absolute = resolve(this.cwd, path);
    let file = this.state.memory.find((v) => v.path === absolute);
    if (!file) { file = { path: absolute, state: 'reported' }; this.state.memory.push(file); }
    Object.assign(file, patch); this.state.memory = this.state.memory.slice(-LIMIT);
  }
  requested(prompt) {
    const words = prompt.split(/\s+/);
    for (const skill of this.state.skills) {
      if (words.includes(`$${skill.name}`) || words[0] === `/${skill.name}`) this.evidence({ id: randomUUID(), kind: 'requested', name: skill.name, detail: 'The user explicitly requested this skill. Loading has not been confirmed.' });
    }
  }
  async catalog() {
    if (this.state.provider !== 'claude') return;
    const home = process.env.CLAUDE_CONFIG_DIR || join(homedir(), '.claude');
    for (const [root, scope] of [[join(this.cwd, '.claude/skills'), 'project'], [join(home, 'skills'), 'user']]) {
      const entries = await readdir(root, { withFileTypes: true }).catch(() => []);
      for (const entry of entries.slice(0, LIMIT)) {
        if (!entry.isDirectory()) continue;
        const path = join(root, entry.name, 'SKILL.md');
        const body = await markdown(path).catch(() => '');
        if (Buffer.byteLength(body) > 128 * 1024) continue;
        const front = body.match(/^---\r?\n([\s\S]*?)\r?\n---/m)?.[1] ?? '';
        const field = (key) => front.match(new RegExp(`^${key}:\\s*(.+)$`, 'm'))?.[1]?.replace(/^["']|["']$/g, '');
        const name = field('name') || entry.name;
        this.skill(name, { path, source: scope, description: field('description'), available: true, estimate: Math.ceil(Buffer.byteLength(body) / 4), user_only: field('disable-model-invocation') === 'true' });
      }
    }
    for (const path of [join(this.cwd, 'CLAUDE.md'), join(this.cwd, '.claude/CLAUDE.md'), join(home, 'CLAUDE.md')]) {
      const meta = await stat(path).catch(() => undefined);
      if (meta?.isFile()) this.memory(path, { state: 'configured', estimate: Math.ceil(meta.size / 4), accuracy: 'estimated' });
    }
  }
  async configuredMemory() {
    const provider = this.state.provider;
    const home = provider === 'codex' ? process.env.CODEX_HOME || join(homedir(), '.codex') : process.env.CLAUDE_CONFIG_DIR || join(homedir(), '.claude');
    const name = provider === 'codex' ? 'AGENTS.md' : 'CLAUDE.md';
    for (const path of [join(this.cwd, name), join(home, name)]) {
      if (this.state.memory.some((m) => m.path === path)) continue;
      const meta = await stat(path).catch(() => undefined);
      if (meta?.isFile()) this.memory(path, { state: 'configured', estimate: Math.ceil(meta.size / 4), accuracy: 'estimated' });
    }
  }
  async preloads(name, id) {
    if (typeof name !== 'string' || !/^[a-zA-Z0-9_-]+$/.test(name)) return;
    const home = process.env.CLAUDE_CONFIG_DIR || join(homedir(), '.claude');
    for (const root of [join(this.cwd, '.claude/agents'), join(home, 'agents')]) {
      const body = await markdown(join(root, `${name}.md`)).catch(() => '');
      if (!body) continue;
      const front = body.match(/^---\r?\n([\s\S]*?)\r?\n---/)?.[1] ?? '';
      const list = front.match(/^skills:\s*\[([^\]]*)\]/m)?.[1];
      for (const raw of list?.split(',') ?? []) {
        const skill = raw.trim().replace(/^["']|["']$/g, '');
        if (!skill) continue;
        this.skill(skill);
        this.evidence({ id, kind: 'preload', name: skill, member: name, detail: 'The member definition declares this preload. Runtime loading was not independently confirmed.' });
      }
      this.update(); return;
    }
  }
  claude(message) {
    const s = this.state;
    if (message.session_id) s.session = message.session_id;
    if (message.type === 'user' && !message.parent_tool_use_id && message.uuid && !array(message.message?.content).some((b) => b.type === 'tool_result')) this.turn = message.uuid;
    if (message.type === 'system' && message.subtype === 'init') {
      s.model = message.model;
      for (const name of array(message.skills)) this.skill(typeof name === 'string' ? name : name.name, { available: true });
    }
    if (message.type === 'system' && message.subtype === 'compact_boundary') {
      s.compacted_at = new Date().toISOString();
      s.used = number(message.compact_metadata?.post_tokens);
      s.categories = [];
      for (const file of s.memory) if (file.state !== 'configured') { file.state = 'historical'; delete file.tokens; }
    }
    if (message.type === 'system' && message.subtype === 'memory_recall') {
      for (const file of array(message.memories)) {
        this.memory(file.path, { source: file.scope, state: 'recalled' });
        this.evidence({ id: message.uuid, kind: 'memory_recall', path: file.path, detail: `Provider memory recall (${message.mode})` });
      }
    }
    if (message.context_usage && !message.parent_tool_use_id) this.report(message.context_usage);
    if (message.type === 'assistant') {
      if (!message.parent_tool_use_id) {
        const u = message.message?.usage;
        if (u?.input_tokens !== undefined) {
          s.used = u.input_tokens + (u.cache_read_input_tokens ?? 0) + (u.cache_creation_input_tokens ?? 0);
          s.usage_accuracy = 'reported'; s.model = message.message.model ?? s.model;
          // Category estimates from an older report no longer describe this request.
          s.categories_stale = true;
        }
      }
      for (const block of array(message.message?.content)) {
        if (block.type !== 'tool_use' || this.calls.has(block.id)) continue;
        const input = block.input ?? {};
        const member = this.members.get(message.parent_tool_use_id) ?? message.subagent_type;
        this.calls.set(block.id, { tool: block.name, input, member });
        if (this.calls.size > 1000) this.calls.delete(this.calls.keys().next().value);
        if (['Task', 'Agent'].includes(block.name)) {
          if (this.members.size >= 1000) this.members.delete(this.members.keys().next().value);
          const name = input.subagent_type ?? input.name;
          this.members.set(block.id, name);
          void this.preloads(name, block.id);
        }
        if (block.name === 'Skill' && typeof input.skill === 'string') {
          this.skill(input.skill);
          this.evidence({ id: block.id, kind: 'invocation', name: input.skill, member, detail: 'Skill tool invoked; selection rationale was not reported.' });
        }
      }
    }
    if (message.type === 'user') {
      for (const block of array(message.message?.content)) {
        if (block.type !== 'tool_result') continue;
        const call = this.calls.get(block.tool_use_id);
        if (!call) continue;
        if (call.tool === 'Skill' && block.is_error) {
          const e = s.evidence.find((e) => e.id === block.tool_use_id && e.kind === 'invocation');
          if (e) { e.kind = 'failed_invocation'; e.detail = 'Skill invocation failed; its instructions may not have loaded.'; }
        }
        if (call.tool === 'Read' && !block.is_error) this.read(call.input.file_path ?? call.input.path, block.tool_use_id, call.member);
      }
    }
  }
  read(path, id, member) {
    if (typeof path !== 'string') return;
    path = resolve(this.cwd, path);
    const skill = this.state.skills.find((s) => s.path === path);
    if (skill || /[/\\]SKILL\.md$/i.test(path)) {
      const name = skill?.name ?? path.split(/[/\\]/).at(-2);
      this.skill(name, { path });
      this.evidence({ id, kind: 'read', name, path, member, detail: 'Successful read of the skill file. The provider did not report its selection rationale.' });
    } else if (this.state.memory.some((f) => f.path === path) || /[/\\](AGENTS|CLAUDE|MEMORY)\.md$/i.test(path)) {
      this.memory(path, { state: 'read' });
      this.evidence({ id, kind: 'memory_read', path, member, detail: 'Successful file read observed.' });
    }
  }
  report(r) {
    const s = this.state;
    s.model = r.model ?? s.model;
    s.used = number(r.totalTokens ?? r.total_tokens);
    s.capacity = number(r.rawMaxTokens ?? r.raw_max_tokens ?? r.maxTokens);
    s.window_kind = 'compaction_window'; s.usage_accuracy = 'provider_estimate';
    s.categories = array(r.categories).filter((c) => ['used', 'free', 'buffer', 'deferred'].includes(c.kind)).slice(0, 60).map((c) => ({ name: c.name, tokens: c.tokens, kind: c.kind, accuracy: 'provider_estimate' }));
    s.categories_stale = false; s.breakdown_at = new Date().toISOString();
    const files = array(r.memoryFiles ?? r.memory_files);
    // A fresh inventory supersedes earlier main-session presence claims; retain them as on-disk sources.
    for (const file of s.memory) if (file.state === 'reported') { file.state = 'configured'; delete file.tokens; }
    for (const file of files) this.memory(file.path, { source: file.type, tokens: file.tokens, accuracy: 'provider_estimate', state: 'reported' });
    for (const skill of s.skills) { skill.advertised = false; delete skill.tokens; }
    for (const skill of array(r.skills?.skillFrontmatter ?? r.skills)) this.skill(skill.name, { source: skill.source, tokens: skill.tokens, accuracy: 'provider_estimate', advertised: true });
    s.reserve = s.categories.filter((c) => c.kind === 'buffer').reduce((n, c) => n + c.tokens, 0);
  }
  codexUsage(p) {
    const usage = p.tokenUsage ?? {};
    const last = usage.last;
    if (last) { this.state.used = number(last.totalTokens ?? ((last.inputTokens ?? 0) + (last.outputTokens ?? 0))); this.state.usage_accuracy = 'reported'; }
    this.state.capacity = number(usage.modelContextWindow) ?? this.state.capacity;
    this.state.window_kind = 'model'; this.turn = p.turnId ?? this.turn;
  }
  codexItem(item, complete) {
    if (item.type === 'contextCompaction' && complete) {
      this.state.compacted_at = new Date().toISOString(); this.state.used = undefined;
    }
    if (!complete) return;
    // Only unambiguous, successful, single read commands are evidence; arbitrary shell text is not.
    if (item.type === 'commandExecution' && item.status === 'completed' && item.exitCode === 0) {
      const command = item.command ?? '';
      const match = command.match(/^\s*cat\s+(?:--\s+)?(?:"([^"\n]+)"|'([^'\n]+)'|([^\s;&|<>`$]+))\s*$/);
      if (match) this.read(match[1] ?? match[2] ?? match[3], item.id);
    }
  }
}
