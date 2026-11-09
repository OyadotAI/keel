// One daemon-owned process per lane. JSON-lines is private to Keel; provider protocols stay here.
import { createInterface } from 'node:readline';
import { readFile } from 'node:fs/promises';
import { resolve, extname } from 'node:path';
import { query } from '@anthropic-ai/claude-agent-sdk';
import { Codex } from './codex.mjs';
import { ContextInspector } from './context.mjs';
import { Requests } from './requests.mjs';
import { ClaudeCompletion } from './claude-completion.mjs';
import { Initialization } from './initialization.mjs';
import { sessionKey } from './session-allow.mjs';

const emit = (event, data) => process.stdout.write(JSON.stringify({ event, data }) + '\n');
const initialization = new Initialization(emit);
let completion;
const requests = new Requests(emit, (event, id, fields) => { if (fields.provider === 'claude') completion?.callback(event, id, fields); });
let provider;
let claude;
let codex;
let session;
let config;
let busy = false;
/** The turn's own prompt has gone in, so a steer lands after it. */
let accepting = false;
/** The effort the live Claude session runs at, so a change is applied once and only when it is one. */
let effort = null;
completion = new ClaudeCompletion((code) => { busy = false; accepting = false; emit('end', { code }); }, (reason) => requests.cancelAll(reason));
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
function ask(tool, command, input, choices, kind = 'permission', signal, agentID) {
  return requests.ask({ tool, command, input, choices, kind, provider, agentID, session_id: session ?? '', lane: config.lane, rules: [] }, signal);
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
const allowedThisSession = new Set();
async function startClaude(q) {
  if (!claude) completion.processStart();
  else completion.begin();
  if (!claude) {
    q = await initialization.launch(q);
    config = q;
    inspector = new ContextInspector('claude', q.cwd, emit);
    await inspector.catalog();
    q.assertLaunch();
    effort = q.effort || null;
    // A new provider session starts with nothing allowed: a rule belongs to the session it was given in.
    allowedThisSession.clear();
    claude = query({ prompt: messages(), options: {
      cwd: q.cwd, pathToClaudeCodeExecutable: q.executable,
      resume: q.session || undefined, model: q.model || undefined, effort: q.effort || undefined,
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
        const key = question ? undefined : sessionKey(tool, input);
        if (key && allowedThisSession.has(key)) return { behavior: 'allow', updatedInput: input };
        let answer;
        try {
          answer = await ask(tool, input.command ?? '', input,
            [{ label: 'Allow once', value: 'allow' }, ...(key ? [{ label: key.startsWith('Bash:') ? 'Allow this command again this session' : `Allow ${key.replace(/^WebFetch:/, '')} for this session`, value: 'session' }] : []), { label: 'Deny', value: 'deny' }], question ? 'question' : 'permission', options.signal, options.agentID);
        } catch (error) { return { behavior: 'deny', message: error.message }; }
        if (question) return { behavior: 'allow', updatedInput: { ...input, answers: answer.answers ?? {} }, toolUseID: options.toolUseID };
        if (answer.decision === 'session' && key) allowedThisSession.add(key);
        return ['allow', 'session'].includes(answer.decision) ? { behavior: 'allow', updatedInput: input } : { behavior: 'deny', message: answer.reason || 'Denied by the user' };
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
            const owner = inspector;
            void contextSummary().then((report) => {
              if (owner === inspector) { owner.report(report); owner.update({}, true); }
            }).catch(() => {
              if (owner === inspector) { owner.limitation('This Claude version could not provide a context breakdown.'); owner.update({}, true); }
            });
          }
          completion.message(message);
        }
      } catch (error) { emit('fatal', String(error)); }
      finally { completion.shutdown(); claude = undefined; }
    })();
  } else {
    await claude.setPermissionMode(q.mode === 'acceptEdits' ? 'acceptEdits' : 'plan');
    await claude.setModel(q.model || undefined);
    // Effort lives in the session's flag layer; null goes back to the model's default.
    if ((q.effort || null) !== effort) {
      await claude.applyFlagSettings({ effortLevel: q.effort || null }).then(() => { effort = q.effort || null; }, (error) => emit('err', `effort not applied: ${error}`));
    }
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
  // Only now can words join the turn: before this they would reach the model ahead of its prompt.
  accepting = true;
}
/** Words typed while a turn runs join that turn, as they do in the CLI — measured: a user message
 *  pushed into the SDK's streaming input mid-turn is read at the next step and answered in the
 *  same result. Refused once the turn has ended: then it would start a turn nobody is listening
 *  to, and the page queues it instead. */
async function steer(prompt) {
  const text = String(prompt ?? '').trim();
  if (!busy || !text) return { ok: false, error: 'No turn is running.' };
  try {
    // After the result, while background agents still hold the turn, the words start the next
    // reply inside the same turn: its output is still attached, and the turn ends when they do.
    if (provider === 'claude' && claude && accepting && !completion.done && !completion.interrupted) {
      completion.steer();
      inputs.push({ type: 'user', message: { role: 'user', content: [{ type: 'text', text }] }, parent_tool_use_id: null, session_id: session ?? '' });
      wake?.(); wake = undefined;
    } else if (provider === 'codex' && codex?.turn) await codex.steer(text);
    else return { ok: false, error: 'This turn is finishing.' };
  } catch (error) { return { ok: false, error: String(error?.message ?? error) }; }
  steers += 1;
  emit('turn', [{ turn: null, op: 'text', step: `steer:${steers}`, kind: 'you', append: text }]);
  return { ok: true };
}
let steers = 0;
async function command(message) {
  if (message.method === 'initialized') { initialization.complete(message); return; }
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
    emit('answer_ack', { id: message.delivery, ...requests.answer(message.id, message.answer) }); return;
  }
  if (message.method === 'interrupt') {
    initialization.cancel('The turn was cancelled.', true);
    if (claude && provider === 'claude' && busy) { completion.interrupt(); await claude.interrupt(); }
    if (codex && provider === 'codex') await codex.interrupt();
    return;
  }
  if (message.method === 'steer') { emit('answer_ack', { id: message.delivery, ...await steer(message.prompt) }); return; }
  if (message.method !== 'start') throw new Error('Unknown runtime command');
  if (busy) throw new Error('A turn is already active');
  busy = true;
  const q = message.query;
  provider = q.provider; config = q;
  try {
    if (provider === 'claude') await startClaude(q);
    else if (provider === 'codex') {
      codex ??= new Codex(emit, ask, daemonDecision, (reason, thread) => requests.cancelAll(reason, thread), (query) => initialization.launch(query));
      await codex.start(q, () => { busy = false; });
    } else throw new Error(`Unsupported provider: ${provider}`);
  } catch (error) {
    // Codex.fail already ended this turn when a pending App Server request rejected.
    if (provider === 'codex' && !busy) return;
    const interrupted = error.name === 'AbortError';
    if (!interrupted) emit('fatal', String(error));
    if (provider === 'claude') { claude?.close(); if (interrupted) completion.complete(-1, 'The turn was cancelled.'); else completion.shutdown('The provider could not start this turn.'); }
    else { busy = false; emit('end', { code: interrupted ? -1 : 1 }); }
  }
}
const rl = createInterface({ input: process.stdin });
rl.on('line', (line) => { try { void command(JSON.parse(line)).catch((error) => emit('fatal', String(error))); } catch (error) { emit('fatal', String(error)); } });
rl.on('close', () => { initialization.cancel('The runtime closed.'); requests.cancelAll('The runtime closed.'); claude?.close(); codex?.close(); process.exit(0); });
