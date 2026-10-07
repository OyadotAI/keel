"""Optional live-provider test: uses authenticated CLIs and consumes model tokens."""
import subprocess, tempfile, os, json, time, urllib.request, socket, sys
from pathlib import Path

workspace = Path(__file__).resolve().parents[3]
repo=tempfile.mkdtemp(prefix='keel-chat-e2e-')
subprocess.run(['git','init','-q',repo],check=True)
subprocess.run(['git','-C',repo,'-c','user.name=Keel Test','-c','user.email=keel@example.invalid','commit','--allow-empty','-qm','fixture'],check=True)
sock=socket.socket();sock.bind(('127.0.0.1',0));port=sock.getsockname()[1];sock.close()
token='keel-chat-fixture-token'
env=os.environ.copy()
env['KEEL_PERMISSIONS_DIR']=tempfile.mkdtemp(prefix='keel-chat-e2e-private-')
for key in list(env):
 if key.startswith('CLAUDE_CODE_') or key in ['CLAUDECODE','CLAUDE_PID']: env.pop(key,None)
p=subprocess.Popen([str(workspace / 'target/debug/keel'),'serve',repo,'--port',str(port),'--exit-on-stdin-eof'],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,text=True,env=env)
p.stdin.write(token+'\n');p.stdin.flush()
def request(path,data=None):
 r=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=json.dumps(data).encode() if data is not None else None,headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
 return urllib.request.urlopen(r,timeout=60)
def replay(lane,after=0):
 response=request('/api/chat/events?lane='+lane+'&after='+str(after));out=[];event=''
 try:
  for raw in response:
   line=raw.decode().strip()
   if line.startswith('event:'):event=line[6:].strip()
   if line.startswith('data:'):
    data=json.loads(line[5:].strip())
    if event=='record':out.append(data)
    if event=='caught-up':return out,data
 finally:response.close()
try:
 for _ in range(100):
  try: request('/api/state').close();break
  except Exception:time.sleep(.1)
 for provider in (sys.argv[1:] or ['claude', 'codex']):
  lane='smoke-'+provider
  query={'id':'first-'+provider,'lane':lane,'provider':provider,'prompt':'Remember the word ORBIT. Reply with exactly KEEL_CHAT_OK. Do not use tools.','mode':'plan','auto_commit':False}
  first=json.load(request('/api/chat/send',query)); duplicate=json.load(request('/api/chat/send',query))
  assert first['accepted'] and not duplicate['accepted'],(first,duplicate)
  time.sleep(1)
  records, status=replay(lane)
  cursor=records[-1]['seq'] if records else 0
  deadline=time.time()+65
  while status['running'] and time.time()<deadline:
   time.sleep(.4);batch,status=replay(lane,cursor);records+=batch
   if batch:cursor=batch[-1]['seq']
  assert not status['running'],'turn timed out'
  assert not [r for r in records if r['event']=='fatal'],[r for r in records if r['event']=='fatal']
  assert 'KEEL_CHAT_OK' in ''.join(f.get('append','') for r in records if r['event']=='turn' for f in r['data']),'missing response'
  assert sum(r['event']=='accepted' for r in records)==1,'duplicate execution'
  session=next(f['id'] for r in records if r['event']=='turn' for f in r['data'] if f['op']=='session')
  query={**query,'id':'second-'+provider,'session':session,'prompt':'What word did I ask you to remember? Reply with just that word. Do not use tools.'}
  json.load(request('/api/chat/send',query));status={'running':True};second=[]
  deadline=time.time()+65
  while status['running'] and time.time()<deadline:
   time.sleep(.4);batch,status=replay(lane,cursor);second+=batch
   if batch:cursor=batch[-1]['seq']
  assert not status['running'],'second turn timed out'
  assert not [r for r in second if r['event']=='fatal'],[r for r in second if r['event']=='fatal']
  assert 'ORBIT' in ''.join(f.get('append','') for r in second if r['event']=='turn' for f in r['data']),'session continuity failed'
  json.load(request('/api/chat/control',{'lane':lane,'method':'close'}))
  print(provider+': durable POST, duplicate suppression, disconnected replay, two-turn continuity, close PASS',flush=True)
finally:
 p.stdin.close()
 try:p.wait(timeout=8)
 except subprocess.TimeoutExpired:p.kill()
