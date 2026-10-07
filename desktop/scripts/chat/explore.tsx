// Full UI, real daemon and PTYs. Only native-shell integration is substituted.
// Launch explore.py first. Credentials are generated locally and never committed.
import config from './.local-config';
const callbacks = new Map<number, unknown>();
let serial = 0;
// Opt-in updater UI fixture: no installed app or daemon is stopped or replaced.
const updateFixture = new URLSearchParams(location.search).get('update') === 'ready';
const updateCalls: string[] = [];
Object.assign(window, { __qaUpdateCalls: updateCalls });
Object.assign(window, { __TAURI_INTERNALS__: {
  metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
  transformCallback(fn: unknown) { const id = ++serial; callbacks.set(id, fn); return id; },
  unregisterCallback(id: number) { callbacks.delete(id); },
  async invoke(command: string, args: Record<string, unknown> = {}) {
    if (updateFixture && command === 'plugin:updater|check') return { rid: 100, version: '99.0.0', currentVersion: '0.3.6', body: 'QA update fixture', rawJson: {} };
    if (updateFixture && command === 'plugin:updater|download') return 101;
    if (updateFixture && command === 'stop_all') { updateCalls.push(command); return; }
    if (updateFixture && command === 'plugin:updater|install') {
      updateCalls.push(command);
      await new Promise(resolve => setTimeout(resolve, 1500));
      throw new Error('QA fixture: installation deliberately blocked');
    }
    if (command === 'open_project' && args.path === config.project) return config.endpoint;
    if (command === 'close_project') return;
    if (command === 'plugin:dialog|open') return config.project;
    if (command === 'plugin:event|listen') return ++serial;
    if (command === 'plugin:event|unlisten') return;
    if (command === 'plugin:notification|is_permission_granted') return false;
    if (command === 'plugin:notification|request_permission') return 'denied';
    if (command === 'plugin:app|version') return '0.3.6';
    throw new Error(`Native operation is outside browser QA: ${command}`);
  },
}, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} } });
const { useStore } = await import('../../src/store');
// Preserve real reload behavior within this disposable repository.
if (useStore.getState().order.some(p => p !== config.project)) {
  useStore.setState({ projects: {}, lanes: {}, order: [], active: undefined });
}
if (!useStore.getState().projects[config.project]) await useStore.getState().openProject(config.project);
await import('../../src/main');

if (updateFixture) await (await import('../../src/update')).check();
