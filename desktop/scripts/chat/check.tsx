// Deterministic browser fixture. Loaded only by Vite at /scripts/chat/check.html.
import React from 'react';
import { createRoot } from 'react-dom/client';
import { useStore } from '../../src/store';
import { applyFrames } from '../../src/reduce';
import { Chat } from '../../src/ui/Chat';
import { Composer } from '../../src/ui/Composer';
import { InterfaceSetting } from '../../src/ui/InterfaceSetting';
import { ApprovalStrip } from '../../src/ui/Approvals';
import { LaneViewSwitch } from '../../src/ui/LaneViewSwitch';
import { App } from '../../src/ui/App';
import '../../src/styles.css';
import '../../src/workspace.css';

const project = '/workspace/keel';
const workspace = new URLSearchParams(location.search).has('workspace');
const preview = new URLSearchParams(location.search).has('preview');
const previewCalls: { command: string; args: unknown }[] = [];
Object.assign(window, { previewCalls });
let conv = { turns: [] } as ReturnType<typeof applyFrames>;
for (let i = 0; i < 300; i++) {
  conv = applyFrames(conv, [
    { op: 'open', turn: `turn-${i}`, prompt: i === 299 ? 'Restore a formatted conversation that stays responsive while three agents are working.' : `Review task ${i + 1}` },
    { op: 'text', turn: `turn-${i}`, step: 'reply', kind: 'say', append: i === 299 ? 'The conversation now stays readable as work progresses.\n\n### What changed\n\n- Messages wrap naturally at the pane width.\n- **Tool activity** expands when you need the details.\n- Your draft and scroll position stay with each lane.\n\n```typescript\nconst followOutput = (atBottom: boolean) =>\n  atBottom ? "auto" : false;\n```\n\n| Workflow | Result |\n| --- | --- |\n| Switch lanes | Running tasks continue |\n| Read earlier messages | Scroll position stays fixed |\n\nTry resizing the window or switching the interface in setup.' : 'Checked the implementation and its tests. The changes are ready for review.' },
    { op: 'call', turn: `turn-${i}`, id: `call-${i}`, tool: 'Bash', subject: 'pnpm test', input: { command: 'pnpm test' }, writes: [] },
    { op: 'result', turn: `turn-${i}`, id: `call-${i}`, state: 'ok', output: 'Tests  42 passed\nDuration  2.4s', cut: 0 },
    { op: 'close', turn: `turn-${i}`, reason: 'done' },
  ]);
}
const lanes = Object.fromEntries(['chat-a','chat-b','chat-c'].map((id) => [id, { id, project, title: 'Formatted conversation', interface: 'chat' as const, agent: 'claude' as const, known: false, isolated: false, jobs: [], queued: [], conv, running: false, loaded: true, pending: [], draft: '' }]));
useStore.setState({ projects: { [project]: { path: project, name: 'Keel', lanes: Object.keys(lanes) } }, lanes, active: 'chat-a', order: [project], ensure: async () => ({ port: 1420, token: 'fixture' }), load: () => {}, send: async (lane, prompt) => {
  const l = useStore.getState().lanes[lane];
  const next = applyFrames(l.conv, [{ op: 'open', turn: `new-${Date.now()}`, prompt }]);
  useStore.setState({ lanes: { ...useStore.getState().lanes, [lane]: { ...l, conv: next, draft: '', running: true } } });
  return true;
} });
function Fixture() {
  const active = useStore((s) => s.active)!;
  const [setup, setSetup] = React.useState(false);
  function question() {
    useStore.setState((s) => ({ lanes: { ...s.lanes, [active]: { ...s.lanes[active], pending: [{ id: 'fixture-question', tool: 'AskUserQuestion', command: '', rules: [], input: { questions: [{ question: 'Which checks should run?', multiSelect: true, options: [{ label: 'Unit tests' }, { label: 'Browser checks' }] }] } }] } } }));
  }
  return <div className="app no-sidebar"><main className="main" style={{ gridColumn: 3 }}><section className="agent-pane">
    <header className="lane-header"><strong className="spacer">Keel QA</strong>{Object.keys(lanes).map((id, i) => <button key={id} onClick={() => useStore.setState({ active: id })}>Lane {i + 1}</button>)}<button onClick={question}>Question</button><button onClick={() => setSetup(!setup)}>Setup</button><LaneViewSwitch lane={active} /></header>
    {setup ? <div className="settings-body"><InterfaceSetting /></div> : <><div className="agents"><Chat key={active} lane={active} /></div><ApprovalStrip lane={active} /><Composer key={`composer-${active}`} lane={active} /></>}
  </section></main></div>;
}
if (workspace) {
  // The full workspace subscribes to native drag/drop events. No native actions
  // run in this fixture; the rest is the application's actual layout and styles.
  Object.assign(window, { __TAURI_INTERNALS__: {
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
    transformCallback: () => 1,
    unregisterCallback: () => {},
    invoke: async (command: string, args: unknown) => {
      if (command.startsWith('preview_')) previewCalls.push({ command, args });
      return 1;
    },
  }, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} } });
  const state = useStore.getState();
  useStore.setState({ opened: ['chat-a'], trusted: { [project]: true }, lanes: {
    ...state.lanes,
    'chat-a': { ...state.lanes['chat-a'], conv: { ...conv, turns: conv.turns.slice(-1) },
      ...(preview ? { dev: { running: true, url: 'http://localhost:3000', command: 'make dev', elsewhere: false, owner: '', log: [] } } : {}),
    },
  }, ...(preview ? { panelTab: 'preview' } : {}) });
}
createRoot(document.getElementById('root')!).render(workspace ? <App /> : <Fixture />);
Object.assign(window, { chatFixture: { useStore, applyFrames } });
