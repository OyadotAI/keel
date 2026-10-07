import { fileURLToPath } from 'node:url';
// Stage a Node runtime alongside the Rust sidecar. End users need neither Node nor npm.
import { createHash } from 'node:crypto';
import { copyFileSync, mkdirSync, mkdtempSync, writeFileSync, readFileSync, renameSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
function stageBinary(source, destination) {
  const temporary = fileURLToPath(destination) + `.staging-${process.pid}`;
  copyFileSync(source, temporary);
  chmodSync(temporary, 0o755);
  renameSync(temporary, destination);
}
const version = '22.22.0';
const native = `${process.arch === 'arm64' ? 'aarch64' : 'x86_64'}-${process.platform === 'darwin' ? 'apple-darwin' : process.platform === 'win32' ? 'pc-windows-msvc' : 'unknown-linux-gnu'}`;
const target = process.argv[2] ?? native;
const out = new URL('../src-tauri/binaries/', import.meta.url);
mkdirSync(out, { recursive: true });
mkdirSync(new URL("../src-tauri/runtime/", import.meta.url), { recursive: true });
if (!process.argv[2]) {
  stageBinary(process.execPath, new URL(`keel-node-${native}${process.platform === 'win32' ? '.exe' : ''}`, out));
} else {
  const targets = target === 'universal-apple-darwin' ? ['aarch64-apple-darwin', 'x86_64-apple-darwin'] : [target];
  const hashes = await (await fetch(`https://nodejs.org/dist/v${version}/SHASUMS256.txt`)).text();
  const stage = mkdtempSync(join(tmpdir(), 'keel-node-'));
  for (const triple of targets) {
    const os = triple.includes('windows') ? 'win' : triple.includes('darwin') ? 'darwin' : 'linux';
    const arch = triple.startsWith('aarch64') ? 'arm64' : 'x64';
    const name = `node-v${version}-${os}-${arch}`;
    const archive = `${name}.${os === 'win' ? 'zip' : 'tar.gz'}`;
    const response = await fetch(`https://nodejs.org/dist/v${version}/${archive}`);
    if (!response.ok) throw new Error(`Could not download ${archive}: ${response.status}`);
    const bytes = Buffer.from(await response.arrayBuffer());
    const expected = hashes.split('\n').find((line) => line.endsWith(`  ${archive}`))?.split(' ')[0];
    if (createHash('sha256').update(bytes).digest('hex') !== expected) throw new Error(`Checksum mismatch for ${archive}`);
    const file = join(stage, archive); writeFileSync(file, bytes);
    // Git Bash can put GNU tar ahead of Windows' ZIP-capable bsdtar. Select the
    // native executable on Windows and use a relative archive path so a drive
    // letter cannot be interpreted as a remote host.
    const tar = process.platform === 'win32'
      ? join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe')
      : 'tar';
    execFileSync(tar, [os === 'win' ? '-xf' : '-xzf', archive], { cwd: stage });
    stageBinary(join(stage, name, os === 'win' ? 'node.exe' : 'bin/node'), new URL(`keel-node-${triple}${os === 'win' ? '.exe' : ''}`, out));
    // Keep the runtime's required notice in the bundle resources.
    writeFileSync(new URL('../src-tauri/runtime/NODE-LICENSE', import.meta.url), readFileSync(join(stage, name, 'LICENSE')));
  }
  if (target === 'universal-apple-darwin') execFileSync('lipo', ['-create', '-output', fileURLToPath(new URL('keel-node-universal-apple-darwin', out)), ...targets.map((t) => fileURLToPath(new URL(`keel-node-${t}`, out)))]);
}
console.log(`Staged agent runtime for ${target}`);
