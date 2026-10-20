import { resolve, extname } from 'node:path';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { ContextInspector } from './context.mjs';

// App Server owns thread state; this adapter maps its items to Keel's ordered conversation ops.
export class Codex {
  constructor(emit, ask, decide) { this.emit = emit; this.ask = ask; this.decide = decide; this.requests = new Map(); this.next = 0; this.text = new Set(); }
  write(value) { this.child.stdin.write(JSON.stringify(value) + '\n'); }
  rpc(method, params, timeoutMs) {
    const id = ++this.next;
    return new Promise((resolve, reject) => {
      const timer = timeoutMs ? setTimeout(() => { this.requests.delete(id); reject(new Error(`${method} timed out`)); }, timeoutMs) : undefined;
      this.requests.set(id, { resolve: (value) => { clearTimeout(timer); resolve(value); }, reject: (error) => { clearTimeout(timer); reject(error); } });
      this.write({ id, method, params });
    });
  }
  ops(...ops) { this.emit('turn', ops.map((op) => ({ turn: null, ...op }))); }
  async start(q, done) {
    this.done = done; this.mode = q.mode;
    if (!this.child) {
      this.child = spawn(q.executable, ['app-server', '--stdio'], { cwd: q.cwd, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
      createInterface({ input: this.child.stdout }).on('line', (line) => {
        try { void this.message(JSON.parse(line)).catch((error) => this.emit('fatal', String(error))); }
        catch (error) { this.emit('fatal', `Invalid App Server message: ${error}`); }
      });
      this.child.stderr.on('data', (bytes) => this.emit('err', bytes.toString()));
      this.child.on('error', (error) => this.fail(error));
      this.child.on('exit', () => this.fail(new Error('Codex App Server exited')));
      await this.rpc('initialize', { clientInfo: { name: 'keel', title: 'Keel', version: '0.3.6' }, capabilities: {} });
      this.write({ method: 'initialized', params: {} });
      const thread = await this.rpc(q.session ? 'thread/resume' : 'thread/start', {
        ...(q.session ? { threadId: q.session } : {}), cwd: q.cwd,
        model: q.model || null, approvalPolicy: 'on-request', sandbox: q.mode === 'acceptEdits' ? 'workspace-write' : 'read-only',
        // Added to Codex's own instructions. `baseInstructions` replaced them: measured, every
        // Keel thread ran without Codex's base prompt and its tool guidance.
        developerInstructions: q.system || null,
      });
      this.thread = thread.thread.id;
      this.inspector = new ContextInspector('codex', q.cwd, this.emit);
      this.inspector.state.session = this.thread;
      this.inspector.state.model = thread.model ?? q.model;
      for (const source of thread.instructionSources ?? thread.thread.instructionSources ?? []) this.inspector.memory(typeof source === 'string' ? source : source.path, { state: 'reported' });
      await this.inspector.configuredMemory();
      try {
        const inventory = await this.rpc('skills/list', { cwds: [q.cwd] }, 3000);
        for (const cwd of inventory.data ?? []) for (const skill of cwd.skills ?? []) {
          this.inspector.skill(skill.name, { path: skill.path, description: skill.description, source: skill.scope, available: skill.enabled !== false, user_only: skill.interface?.policy?.allowImplicitInvocation === false });
        }
      } catch { this.inspector.limitation('This Codex version did not provide a skill inventory.'); }
      this.inspector.limitation('Codex does not report a context category breakdown or an explanation of implicit skill selection.');
      this.inspector.update();
      this.ops({ op: 'session', id: this.thread, commands: [] });
      const models = await this.rpc('model/list', {});
      this.emit('capabilities', { models: models.data, commands: [] });
      if (q.session && thread.thread.turns?.length) this.emit('snapshot', { frames: [...historyFrames(thread.thread.turns), { turn: null, op: 'open', prompt: q.prompt }] });
    }
    this.text.clear();
    this.inspector.requested(q.prompt);
    const result = await this.rpc('turn/start', {
      threadId: this.thread, input: [{ type: 'text', text: q.prompt }, ...(q.attachments ?? []).map((a) => /\.(png|jpe?g|webp|gif)$/i.test(extname(a.path)) ? { type: 'localImage', path: resolve(q.cwd, a.path) } : { type: 'text', text: `Attached file: ${resolve(q.cwd, a.path)}` })],
      model: q.model || null, approvalPolicy: 'on-request',
      sandboxPolicy: q.mode === 'acceptEdits' ? { type: 'workspaceWrite', writableRoots: [q.cwd], networkAccess: false, excludeTmpdirEnvVar: false, excludeSlashTmp: false } : { type: 'readOnly' },
    });
    this.turn = result.turn.id;
  }
  fail(error) {
    for (const pending of this.requests.values()) pending.reject(error);
    this.requests.clear(); this.child = undefined;
    this.emit('fatal', String(error)); this.emit('end', { code: 1 }); this.done?.();
  }
  async message(message) {
    if (message.id !== undefined && !message.method) {
      const pending = this.requests.get(message.id); this.requests.delete(message.id);
      if (message.error) pending?.reject(new Error(message.error.message)); else pending?.resolve(message.result);
      return;
    }
    const p = message.params ?? {};
    if (message.id !== undefined) {
      let result;
      if (message.method === 'item/tool/requestUserInput') {
        const answer = await this.ask('AskUserQuestion', '', p, [], 'question');
        result = { answers: Object.fromEntries((p.questions ?? []).map((q) => [q.id, { answers: [answer.answers?.[q.id] ?? answer.answers?.[q.question] ?? ''] }])) };
      } else if (['item/commandExecution/requestApproval', 'item/fileChange/requestApproval'].includes(message.method)) {
        const choices = (p.availableDecisions ?? ['accept', 'acceptForSession', 'decline', 'cancel']).map((value) => ({ value, label: typeof value === 'string' ? ({ accept: 'Allow once', acceptForSession: 'Allow for session', decline: 'Deny', cancel: 'Cancel turn' }[value] ?? value) : 'Allow with proposed rule' }));
        // A command the sandbox refused is asked of Keel first: trust and saved rules answer it,
        // or Keel's card asks with "always". File changes and a read-only lane keep this card.
        const command = script(p.command);
        const decided = message.method.includes('commandExecution') && p.kind !== 'writeStdin' && this.mode === 'acceptEdits' && command
          ? await this.decide('Bash', { command }, p.approvalId ?? p.itemId, 'codex', this.thread) : undefined;
        if (decided === 'allow') result = { decision: 'accept' };
        else if (decided) result = { decision: 'decline' };
        else {
          const answer = await this.ask(message.method.includes('fileChange') ? 'File changes' : 'Command', p.command ?? '', p, choices);
          result = { decision: answer.decision };
        }
      } else if (message.method === 'mcpServer/elicitation/request') {
        const answer = await this.ask('MCP input', p.message ?? '', p, [{ label: 'Accept', value: 'accept' }, { label: 'Decline', value: 'decline' }, { label: 'Cancel', value: 'cancel' }], 'elicitation');
        result = { action: answer.decision, content: answer.content ?? null };
      } else {
        // Never invent an approval for an unknown protocol request.
        this.write({ id: message.id, error: { code: -32601, message: `Keel has no handler for ${message.method}` } });
        this.emit('fatal', `An unsupported Codex request needs attention: ${message.method}`); return;
      }
      this.write({ id: message.id, result }); return;
    }
    const method = message.method;
    // A spawned member's thread reports on this connection too. Its turn ending is not this
    // turn ending, and its words are not this agent's. Requests above stay answered whatever the
    // thread, or the member waits on an approval nobody can see.
    if (p.threadId && this.thread && p.threadId !== this.thread) return;
    if (method === 'item/agentMessage/delta' || method === 'item/reasoning/summaryTextDelta' || method === 'item/reasoning/textDelta') {
      const key = `${p.itemId}:${method.includes('reasoning') ? 'think' : 'say'}`;
      if (typeof p.delta === 'string' && p.delta.length) {
        this.text.add(key); this.ops({ op: 'text', step: key, kind: method.includes('reasoning') ? 'think' : 'say', append: p.delta });
      }
    } else if (method === 'item/started' || method === 'item/completed') {
      this.item(p.item, method === 'item/completed');
      this.inspector?.codexItem(p.item, method === 'item/completed'); this.inspector?.update();
    }
    else if (method === 'turn/started') { this.turn = p.turn.id; if (this.inspector) this.inspector.turn = p.turn.id; }
    else if (method === 'turn/completed') {
      this.inspector?.update({}, true);
      if (p.turn.error) this.emit('fatal', p.turn.error.message);
      this.emit('end', { code: p.turn.status === 'completed' ? 0 : p.turn.status === 'interrupted' ? -1 : 1 });
      this.done?.(); this.turn = undefined;
    } else if (method === 'error' && !p.willRetry) this.emit('fatal', p.error?.message ?? 'Codex reported an error');
    else if (method === 'thread/tokenUsage/updated') {
      this.inspector?.codexUsage(p); this.inspector?.update();
      const usage = p.tokenUsage.last ?? p.tokenUsage.total;
      this.emit('msg', { type: 'turn.completed', usage: { input_tokens: usage.inputTokens, output_tokens: usage.outputTokens, cached_input_tokens: usage.cachedInputTokens, context_tokens: this.inspector?.state.used, model_context_window: p.tokenUsage.modelContextWindow } });
    }
  }
  item(item, complete) {
    const type = item.type;
    if (type === 'agentMessage' || type === 'reasoning') {
      const kind = type === 'reasoning' ? 'think' : 'say';
      const text = itemText(item);
      if (complete && text.trim() && !this.text.has(`${item.id}:${kind}`)) this.ops({ op: 'text', step: `${item.id}:${kind}`, kind, append: text });
      return;
    }
    if (type === 'userMessage') return;
    const tool = { commandExecution: 'Bash', fileChange: 'Edit', mcpToolCall: item.tool, webSearch: 'Web search', collabAgentToolCall: 'Subagent' }[type] ?? type;
    this.ops({ op: 'call', id: item.id, tool, subject: item.command ?? item.query ?? item.tool ?? type, input: item, writes: (item.changes ?? []).map((c) => c.path) });
    if (complete) this.ops({ op: 'result', id: item.id, state: item.status === 'failed' || item.status === 'declined' ? 'failed' : 'ok', output: item.aggregatedOutput ?? JSON.stringify(item.result ?? item.changes ?? ''), cut: 0 });
  }
  interrupt() { return this.turn ? this.rpc('turn/interrupt', { threadId: this.thread, turnId: this.turn }) : Promise.resolve(); }
  steer(prompt) { return this.rpc('turn/steer', { threadId: this.thread, expectedTurnId: this.turn, input: [{ type: 'text', text: prompt }] }); }
  close() { this.child?.kill(); }
}

/// The command Codex will run, without the shell it wraps every command in. Codex sends
/// `/bin/zsh -lc 'git status'` (or its argv); read whole, the program is `zsh`, no saved rule ever
/// matches, and "always" would save `Bash(zsh *)` — every command there is. Anything that does not
/// unwrap cleanly is undefined, so Codex's own card asks instead.
const SHELLS = new Set(['/bin/sh', '/bin/bash', '/bin/zsh', '/usr/bin/bash', '/usr/bin/zsh']);
export function script(command) {
  const plain = /^[A-Za-z0-9_\-./:@,+%=]+$/;
  // The system's shells, by exact path. A pattern let `./tools/zsh -lc 'git status'` — a binary
  // the repository ships — read as `git status` and pass a `Bash(git *)` rule.
  const shell = { test: (p) => SHELLS.has(p) };
  if (Array.isArray(command)) {
    if (command.length === 3 && shell.test(command[0]) && ['-c', '-lc'].includes(command[1])) return command[2] || undefined;
    return command.length && command.every((w) => plain.test(w)) ? command.join(' ') : undefined;
  }
  if (typeof command !== 'string' || !command.trim()) return undefined;
  const wrapped = command.match(/^(\S+) (-l?c) ([\s\S]*)$/);
  if (!wrapped) return command;
  // Shaped like a wrapper around a shell this cannot vouch for: Codex's card.
  if (!shell.test(wrapped[1])) return undefined;
  const body = wrapped[3];
  if (/^'[^']*'$/.test(body)) return body.slice(1, -1) || undefined;
  if (/^"[^"$`\\]*"$/.test(body)) return body.slice(1, -1) || undefined;
  // Unquoted, `-c` takes one word and the rest become `$0`, `$1`: one word or nothing.
  return plain.test(body) ? body : undefined;
}

export function historyFrames(turns) {
  const frames = [];
  for (const turn of turns) {
    const user = turn.items.find((item) => item.type === 'userMessage');
    const prompt = (user?.content ?? []).filter((c) => c.type === 'text').map((c) => c.text).join('\n');
    frames.push({ turn: turn.id, op: 'open', prompt });
    const decoder = new Codex((event, ops) => { if (event === 'turn') frames.push(...ops.map((op) => ({ ...op, turn: turn.id }))); }, () => {});
    for (const item of turn.items) decoder.item(item, true);
    frames.push({ turn: turn.id, op: 'close', reason: turn.status === 'completed' ? 'done' : turn.status === 'interrupted' ? 'interrupted' : 'failed' });
  }
  return frames;
}

// Completed reasoning can have an empty summary alongside available display text.
// Never create expandable blocks from absent text or encrypted provider payloads.
function itemText(item) {
  const plain = (value) => typeof value === 'string' ? value : Array.isArray(value) ? value.map((part) => typeof part === 'string' ? part : typeof part?.text === 'string' ? part.text : '').filter(Boolean).join('\n') : '';
  return [item.text, item.summary, item.content].map(plain).find((text) => text.trim()) ?? '';
}
