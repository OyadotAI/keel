// Generate the private WK runtime from Oya's canonical semantics with all annotation/recording setup disabled.
import {build} from 'esbuild';
import {resolve,join} from 'node:path';
const source=process.argv[2];
if(!source)throw Error('Pass the Oya Browser checkout path to generate the shared diagnostic runtime.');
await build({entryPoints:[join(resolve(source),'browser/src/preload/debug-webkit.ts')],bundle:true,platform:'browser',format:'iife',target:'safari17',define:{__OYA_ANALYZER_SEMANTICS_ONLY__:'true'},banner:{js:'// Browser-owned WK diagnostic runtime. Regenerate with desktop/scripts/browser-debug/generate.mjs <Oya checkout>.'},outfile:resolve(import.meta.dirname,'../../src-tauri/src/browser/debug-runtime.js')});
