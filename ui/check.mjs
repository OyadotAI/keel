// Render Keel's side panels in a headless DOM and print what they produce.
//
//   bun ui/check.mjs [base-url]        (default http://localhost:7777)
//
// Not a substitute for looking at the thing. It is the answer to a narrower question — does this
// code run, and does it produce the elements it is supposed to — which is what every UI bug in
// this project so far has actually been. A duplicated block that declared `let` twice and blanked
// the page; markup appended after the script that bound to it, leaving every button inert. Both
// were invisible until someone opened the application, and both would have shown up here.
//
// happy-dom does not execute <script> elements, so the page's code is run through node:vm with
// the Window as the context. One context is one lexical scope, which is the only way to reach
// `const PANELS` — the same way a second <script> reaches it in a browser.
import { Window } from 'happy-dom';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const BASE = process.argv[2] ?? 'http://localhost:7777';
const html = readFileSync(new URL('./index.html', import.meta.url), 'utf8');
const script = html.split('<script>').pop().split('</script>')[0];

const w = new Window({ url: BASE, width: 1440, height: 900 });
const d = w.document;
d.body.innerHTML = html.split('<body>')[1].split('<script')[0];

// Stubs for what the page reaches for but this harness has no business running.
w.require = () => {};
w.monaco = undefined;
w.Terminal = function () { throw new Error('no terminal here'); };
w.FitAddon = { FitAddon: function () {} };
w.WebSocket = function () { return { readyState: 0, send() {}, close() {} }; };
w.EventSource = function () { return { addEventListener() {}, close() {} }; };
w.matchMedia = () => ({ matches: false, addEventListener() {}, addListener() {} });
w.fetch = (u, o) => fetch(u.startsWith('http') ? u : BASE + u, o);

// happy-dom does not execute <script> elements, so the page's code is run against its Window as
// a vm context. One context means one lexical scope, which is how `const PANELS` becomes reachable
// — the same way a second <script> would reach it in a browser.
// The Window is not a JavaScript global object: it has no Map, no Promise, no JSON. Seed the
// ones the page uses before it becomes the vm's global.
for (const k of ['Map','Set','WeakMap','WeakSet','Promise','JSON','Math','Date','Array','Object',
                 'String','Number','Boolean','RegExp','Error','TypeError','Symbol','Intl','Proxy',
                 'Reflect','BigInt','ArrayBuffer','Uint8Array','TextEncoder','TextDecoder',
                 'URLSearchParams','URL','encodeURIComponent','decodeURIComponent','parseInt',
                 'parseFloat','isNaN','isFinite','structuredClone','console','queueMicrotask'])
  if (w[k] === undefined) w[k] = globalThis[k];
w.globalThis = w;

const ctx = vm.createContext(w);
try {
  vm.runInContext(script +
    '\n;globalThis.__t = { PANELS, askFix, mcpAddForm, topicShift, learnTopic, resetTopic };',
    ctx, { filename: 'ui/index.html' });
} catch (e) { console.log('RUN ERROR: ' + e.message); }

// `state` and `git` are `let` bindings inside the script, not window properties, so they are
// assigned in the same context rather than set on the Window.
const state = await (await fetch(BASE + '/api/state')).json();
const gitStatus = await (await fetch(BASE + '/api/git/status')).json();
vm.runInContext('state = ' + JSON.stringify(state) +
  '; git = ' + JSON.stringify(gitStatus) + ';', ctx);

const strip = s => s
  .replace(/<[^>]+>/g, m => (m.startsWith('</') ? '' : ''))
  .split('').map(t => t.trim()).filter(Boolean);

let failed = 0;

for (const name of ['readiness', 'skills', 'mcp', 'agents', 'hooks', 'changes', 'problems']) {
  console.log('\n=== ' + name.toUpperCase() + ' ' + '='.repeat(40));
  try {
    w.__t.PANELS[name]();
    const side = d.getElementById('side');
    console.log(strip(side.innerHTML).join('\n'));
    console.log('-- elements: ' + ['.sec', '.prow', '.fnd', '.verdict', '.act', '.kebab']
      .map(sel => sel + ' x' + side.querySelectorAll(sel).length).join('   '));
  } catch (e) { console.log('THREW: ' + e.message); failed++; }
}

// The add-a-server form is the one panel control that builds its own UI, so it is exercised too.
console.log('\n=== MCP ADD FORM ' + '='.repeat(33));
try {
  w.__t.PANELS.mcp();
  w.__t.mcpAddForm();
  const form = d.querySelector('.addform');
  if (!form) throw new Error('no form rendered');
  console.log('fields: ' + [...form.querySelectorAll('input,textarea')]
    .map(i => i.id).join(', '));
  console.log('transports: ' + [...form.querySelectorAll('[data-t]')]
    .map(b => b.dataset.t).join(', '));
  console.log('actions: ' + [...form.querySelectorAll('.acts button')]
    .map(b => b.textContent).join(', '));
} catch (e) { console.log('THREW: ' + e.message); failed++; }

// ── the topic-shift detector ────────────────────────────────────────────────────────────────
// Its whole job is to stay quiet. A false positive interrupts someone mid-thought; a false
// negative is only what happens today. So the quiet cases are the ones that matter here, and they
// are asserted rather than eyeballed.
console.log('\n=== TOPIC SHIFT ' + '='.repeat(34));
{
  const now = new Date().toISOString();
  const s = { id: 's1', title: 'redesign the readiness panel', messages: 40, last_active: now };
  vm.runInContext('state={workspace:{sessions:[' + JSON.stringify(s) + ']}}; sessionId="s1";', ctx);
  vm.runInContext('resetTopic();', ctx);
  for (const t of [
    'redesign the readiness panel, it is busy and the priority is not obvious',
    'the blocking findings should get more room than the medium ones',
    'use a dot instead of a chip for severity',
    'the score needs a verdict sentence beside it',
    'crates/keel/src/serve.rs readiness scan findings severity',
  ]) w.__t.learnTopic(t);

  const quiet = [
    'make the blocking section stand out more',
    'also add a count to the section header',
    'the severity dot colour is wrong for medium',
    'fix it',
    'now do the same for the skills panel',
    'that looks better, tighten the spacing',
    'undo that',
  ];
  const ask = [
    'deploy the worker to cloudflare and check the wrangler bindings',
    'write a github actions workflow that runs terraform plan on pull requests',
    'the docker container for postgres keeps restarting, look at the compose file',
  ];

  let bad = 0;
  for (const p of quiet) if (w.__t.topicShift(p)) { console.log('  FALSE POSITIVE: ' + p); bad++; }
  for (const p of ask) if (!w.__t.topicShift(p)) { console.log('  MISSED: ' + p); bad++; }

  // A stale session is not on its own a reason to ask; the subject has to have moved too.
  const stale = { ...s, last_active: new Date(Date.now() - 12 * 3.6e6).toISOString() };
  vm.runInContext('state={workspace:{sessions:[' + JSON.stringify(stale) + ']}};', ctx);
  if (w.__t.topicShift('the severity dot colour is wrong for medium')) {
    console.log('  FALSE POSITIVE: a related prompt in a stale session'); bad++;
  }

  // And a session with barely any history has nothing to judge against.
  vm.runInContext('state={workspace:{sessions:[' + JSON.stringify(s) + ']}}; resetTopic();', ctx);
  w.__t.learnTopic('readiness panel');
  if (w.__t.topicShift('deploy the worker to cloudflare and check the wrangler bindings')) {
    console.log('  FALSE POSITIVE: judged a session with almost no history'); bad++;
  }

  failed += bad;
  console.log(bad ? bad + ' wrong' : '  ' + quiet.length + ' quiet, ' + ask.length + ' asked, correctly');
}

console.log(failed ? '\n' + failed + ' check(s) failed' : '\nAll panels rendered.');

// The page leaves timers and sockets open; nothing here is waiting on them.
process.exit(failed ? 1 : 0);
