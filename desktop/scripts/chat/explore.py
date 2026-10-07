"""Launch the real Keel UI against a disposable repo. Uses authenticated provider CLIs.
Run with Vite already serving port 1420. Ctrl+C stops the test daemon.
"""
import json, os, secrets, socket, subprocess, tempfile, urllib.request
from pathlib import Path

root = Path(__file__).resolve().parents[3]
repo = Path(tempfile.mkdtemp(prefix='keel-explore-'))
(repo / 'README.md').write_text('# Keel exploratory QA\n\nA disposable project for real UI testing.\n')
(repo / 'hello.py').write_text('def greet(name):\n    return f"Hello, {name}!"\n')
(repo / 'Makefile').write_text('check:\n\tpython3 -m py_compile hello.py\n')
subprocess.run(['git', 'init', '-q', str(repo)], check=True)
subprocess.run(['git', '-C', str(repo), 'add', '.'], check=True)
subprocess.run(['git', '-C', str(repo), '-c', 'user.name=Keel QA', '-c', 'user.email=qa@example.invalid', 'commit', '-qm', 'Seed QA project'], check=True)
sock = socket.socket(); sock.bind(('127.0.0.1', 0)); port = sock.getsockname()[1]; sock.close()
token = secrets.token_hex(32)
env = {k: v for k, v in os.environ.items() if not k.startswith('CLAUDE_CODE_') and k not in ['CLAUDECODE', 'CLAUDE_PID']}
env['KEEL_PERMISSIONS_DIR'] = tempfile.mkdtemp(prefix='keel-explore-private-')
config = Path(__file__).with_name('.local-config.ts')
config.write_text('export default ' + json.dumps({'project': str(repo), 'endpoint': {'port': port, 'token': token}}) + ';\n')
config.chmod(0o600)
log = open('/private/tmp/keel-explore-daemon.log', 'w')
p = subprocess.Popen([str(root / 'target/debug/keel'), 'serve', str(repo), '--port', str(port), '--no-open', '--exit-on-stdin-eof'], stdin=subprocess.PIPE, stdout=log, stderr=log, text=True, env=env)
p.stdin.write(token + '\n'); p.stdin.flush()
print('QA project:', repo, flush=True)
print('Open http://127.0.0.1:1420/scripts/chat/explore.html', flush=True)
try:
    p.wait()
except KeyboardInterrupt:
    p.stdin.close()
    try: p.wait(timeout=8)
    except subprocess.TimeoutExpired: p.kill()
finally:
    config.unlink(missing_ok=True)
