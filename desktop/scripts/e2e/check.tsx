// Live-daemon browser entry point. Only the native shell is adapted; store actions,
// HTTP, event streams, provider runtime, hooks, and UI are the production code.
import React from 'react';
import { createRoot } from 'react-dom/client';
import { App } from '../../src/ui/App';
import { useStore } from '../../src/store';
import '../../src/styles.css';
import '../../src/workspace.css';
import '../../src/ide.css';

const config = JSON.parse(sessionStorage.getItem('keel.e2e') ?? 'null');
if (!config) throw new Error('Set keel.e2e in sessionStorage to a temporary project and daemon endpoint.');
const nativeCalls: { command: string; args: unknown }[] = [];
Object.assign(window, {
  e2e: { useStore, nativeCalls },
  __TAURI_INTERNALS__: {
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
    transformCallback: () => 1,
    unregisterCallback: () => {},
    invoke: async (command: string, args: unknown) => {
      nativeCalls.push({ command, args });
      if (command === 'open_project') return config.endpoint;
      if (command.startsWith('plugin:') || command.startsWith('preview_')) return null;
      throw new Error(`Unimplemented native command: ${command}`);
    },
  },
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
});
useStore.setState({ chatInterface: 'chat' });
await useStore.getState().openProject(config.project);
createRoot(document.getElementById('root')!).render(<App />);
