// One daemon-owned process per lane. JSON-lines is private to Keel; provider protocols stay here.
import { createInterface } from 'node:readline';
import { readFile } from 'node:fs/promises';
import { resolve, extname } from 'node:path';
import { randomUUID } from 'node:crypto';
import { query } from '@anthropic-ai/claude-agent-sdk';
import { Codex } from './codex.mjs';
import { ContextInspector } from './context.mjs';

const emit = (event, data) => process.stdout.write(JSON.stringify({ event, data }) + '\n');
const pending = new Map();
let provider;
let claude;
let codex;
let session;
let config;
let busy = false;
let inspector;
let contextRead;
function contextSummary() {
  if (contextRead) return contextRead;
  let timer;
  contextRead = Promise.race([
    claude.getContextUsage({ detail: 'summary' }),
    new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('Context report timed out')), 4000); }),
  ]).finally(() => { clearTimeout(timer); contextRead = undefined; });
  return contextRead;
}
const inputs = [];
let wake;
async function* messages() {
  for (;;) {
    if (!inputs.length) await new Promise((resolve) => { wake = resolve; });
    while (inputs.length) yield inputs.shift();
  }
}
function ask(tool, command, input, choices, kind = 'permission') {
  const id = randomUUID();
  emit('request', { id, tool, command, input, choices, kind, provider, session_id: session ?? '', lane: config.lane, rules: [] });
  return new Promise((resolve) => pending.set(id, resolve)).finally(() => emit('resolved', { id }));
}
// 'allow', a refusal ({reason}), or undefined when the daemon could not be asked — in which case
// the local card asks instead, so this never fails open and never waits forever.
async function daemonDecision(tool, input, toolUseID, from = 'claude-sdk', thread = session) {
  if (!config?.port) return undefined;
  try {
    const res = await fetch(`http://127.0.0.1:${config.port}/api/approve/ask`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ tool_name: tool, tool_input: input, tool_use_id: toolUseID ?? '', session_id: thread ?? '', lane: config.lane, cwd: config.cwd, provider: from }),
      // Held open while the person decides; the daemon's own wait is four minutes.
      signal: AbortSignal.timeout(300_000),
    });
    if (!res.ok) return undefined;
    const d = await res.json();
    if (d.decision === 'allow') return 'allow';
    if (d.decision === 'deny') return { reason: d.reason };
  } catch { /* the local card asks */ }
  return undefined;
}
async function startClaude(q) {
  if (!claude) {
    config = q;
    inspector = new ContextInspector('claude', q.cwd, emit);
    await inspector.catalog();
    claude = query({ prompt: messages(), options: {
      cwd: q.cwd, pathToClaudeCodeExecutable: q.executable,
      resume: q.session || undefined, model: q.model || undefined,
      permissionMode: q.mode === 'acceptEdits' ? 'acceptEdits' : 'plan',
      settingSources: ['user', 'project', 'local'],
      includePartialMessages: true,
      systemPrompt: { type: 'preset', preset: 'claude_code', append: q.system },
      settings: q.settings,
      additionalDirectories: q.repo !== q.cwd ? [q.repo] : [],
      mcpServers: q.mcp?.mcpServers,
      stderr: (line) => emit('err', line),
      canUseTool: async (tool, input, options) => {
        const question = tool === 'AskUserQuestion';
        // The daemon holds trust, saved and session rules, and a card that can say "always". This
        // allowlist was frozen when the lane started, and this card could only say "once".
        // A plan is the person's to approve, and a planning lane's edits and commands stay theirs
        // to allow; trust covers its MCP tools.
        if (!question && tool !== 'ExitPlanMode' && (config.mode === 'acceptEdits' || tool.startsWith('mcp__'))) {
          const decided = await daemonDecision(tool, input, options.toolUseID);
          if (decided === 'allow') return { behavior: 'allow', updatedInput: input };
          if (decided) return { behavior: 'deny', message: decided.reason || 'Not approved in Keel.' };
        }
        const answer = await ask(tool, input.command ?? '', input,
          [{ label: 'Allow once', value: 'allow' }, { label: 'Deny', value: 'deny' }], question ? 'question' : 'permission');
        if (question) return { behavior: 'allow', updatedInput: { ...input, answers: answer.answers ?? {} }, toolUseID: options.toolUseID };
        return answer.decision === 'allow' ? { behavior: 'allow', updatedInput: input } : { behavior: 'deny', message: answer.reason || 'Denied by the user' };
      },
    }});
    void (async () => {
      try {
        for await (const message of claude) {
          if (message.session_id) session = message.session_id;
          inspector.claude(message);
          if (message.type !== 'stream_event') inspector.update({}, message.type === 'result');
          emit('msg', message);
          if (message.type === 'system' && message.subtype === 'init') {
            const [commands, models] = await Promise.all([claude.supportedCommands(), claude.supportedModels()]);
            emit('capabilities', { commands, models });
          }
          if (message.type === 'result') {
            busy = false; emit('end', { code: message.is_error ? 1 : 0 });
            const owner = inspector;
            void contextSummary().then((report) => {
              if (owner === inspector) { owner.report(report); owner.update({}, true); }
            }).catch(() => {
              if (owner === inspector) { owner.limitation('This Claude version could not provide a context breakdown.'); owner.update({}, true); }
            });
          }
        }
      } catch (error) { emit('fatal', String(error)); }
      finally { claude = undefined; if (busy) { busy = false; emit('end', { code: 1 }); } }
    })();
  } else {
    await claude.setPermissionMode(q.mode === 'acceptEdits' ? 'acceptEdits' : 'plan');
    await claude.setModel(q.model || undefined);
  }
  inspector.requested(q.prompt);
  const content = [{ type: 'text', text: q.prompt }];
  for (const a of q.attachments ?? []) {
    const path = resolve(q.cwd, a.path);
    const mime = a.mime || ({ '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.webp': 'image/webp', '.gif': 'image/gif' }[extname(path).toLowerCase()]);
    if (['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(mime)) content.push({ type: 'image', source: { type: 'base64', media_type: mime, data: (await readFile(path)).toString('base64') } });
    else content.push({ type: 'text', text: `Attached file: ${path}` });
  }
  inputs.push({ type: 'user', message: { role: 'user', content }, parent_tool_use_id: null, session_id: session ?? '' });
  wake?.(); wake = undefined;
}
async function command(message) {
  if (message.method === 'inspect_context') {
    try {
      if (claude && inspector) {
        inspector.report(await contextSummary());
        emit('inspection', { id: message.id, snapshot: { ...inspector.state, updated_at: new Date().toISOString() } });
      } else if (codex?.inspector) emit('inspection', { id: message.id, snapshot: codex.inspector.state });
      else emit('inspection', { id: message.id, error: 'No provider session is attached to this runtime.' });
    } catch { emit('inspection', { id: message.id, error: 'The provider could not report context usage. Saved observations remain available.' }); }
    return;
  }
  if (message.method === 'answer') {
    const resolve = pending.get(message.id);
    if (!resolve) throw new Error('That request is no longer pending');
    pending.delete(message.id); resolve(message.answer); return;
  }
  if (message.method === 'interrupt') { if (claude) await claude.interrupt(); if (codex) await codex.interrupt(); return; }
  if (message.method === 'steer') { if (!codex) throw new Error('Steering is not available in this session'); await codex.steer(message.prompt); return; }
  if (message.method !== 'start') throw new Error('Unknown runtime command');
  if (busy) throw new Error('A turn is already active');
  busy = true;
  const q = message.query;
  provider = q.provider; config = q;
  try {
    if (provider === 'claude') await startClaude(q);
    else if (provider === 'codex') {
      codex ??= new Codex(emit, ask, daemonDecision);
      await codex.start(q, () => { busy = false; });
    } else throw new Error(`Unsupported provider: ${provider}`);
  } catch (error) { busy = false; emit('fatal', String(error)); emit('end', { code: 1 }); }
}
const rl = createInterface({ input: process.stdin });
rl.on('line', (line) => { try { void command(JSON.parse(line)).catch((error) => emit('fatal', String(error))); } catch (error) { emit('fatal', String(error)); } });
rl.on('close', () => { claude?.close(); codex?.close(); process.exit(0); });
