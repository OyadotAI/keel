import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { mkdirSync, copyFileSync } from 'node:fs';
mkdirSync(new URL('../src-tauri/runtime/', import.meta.url), { recursive: true });
await build({ entryPoints: [fileURLToPath(new URL('../runtime/bridge.mjs', import.meta.url))], bundle: true, platform: 'node', format: 'esm', target: 'node22', outfile: fileURLToPath(new URL('../src-tauri/runtime/keel-runtime.mjs', import.meta.url)), banner: { js: "import { createRequire as __keelRequire } from 'node:module'; const require = __keelRequire(import.meta.url);" } });

copyFileSync(new URL("../node_modules/@anthropic-ai/claude-agent-sdk/LICENSE.md", import.meta.url), new URL("../src-tauri/runtime/CLAUDE-SDK-LICENSE.md", import.meta.url));
