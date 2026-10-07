// One daemon-owned process per lane. JSON-lines is private to Keel; provider protocols stay here.
import { createInterface } from 'node:readline';
import { readFile } from 'node:fs/promises';
import { resolve, extname } from 'node:path';
import { randomUUID } from 'node:crypto';
import { query } from '@anthropic-ai/claude-agent-sdk';
import { Codex } from './codex.mjs';

const emit = (event, data) => process.stdout.write(JSON.stringify({ event, data }) + '\n');
const pending = new Map();
let provider;
let claude;
let codex;
let session;
let config;
let busy = false;
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
async function startClaude(q) {
  if (!claude) {
    config = q;
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
          emit('msg', message);
          if (message.type === 'system' && message.subtype === 'init') {
            const [commands, models] = await Promise.all([claude.supportedCommands(), claude.supportedModels()]);
            emit('capabilities', { commands, models });
          }
          if (message.type === 'result') { busy = false; emit('end', { code: message.is_error ? 1 : 0 }); }
        }
      } catch (error) { emit('fatal', String(error)); }
      finally { claude = undefined; if (busy) { busy = false; emit('end', { code: 1 }); } }
    })();
  } else {
    await claude.setPermissionMode(q.mode === 'acceptEdits' ? 'acceptEdits' : 'plan');
    await claude.setModel(q.model || undefined);
  }
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
      codex ??= new Codex(emit, ask);
      await codex.start(q, () => { busy = false; });
    } else throw new Error(`Unsupported provider: ${provider}`);
  } catch (error) { busy = false; emit('fatal', String(error)); emit('end', { code: 1 }); }
}
const rl = createInterface({ input: process.stdin });
rl.on('line', (line) => { try { void command(JSON.parse(line)).catch((error) => emit('fatal', String(error))); } catch (error) { emit('fatal', String(error)); } });
rl.on('close', () => { claude?.close(); codex?.close(); process.exit(0); });
