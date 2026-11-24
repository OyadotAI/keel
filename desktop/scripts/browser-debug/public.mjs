// Run the real embedded browser_act surface and remove only its observed store after WebKit exits.
import {mkdtemp,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn} from 'node:child_process';
import http from 'node:http';
const scratch=await mkdtemp(join(tmpdir(),'oya-keel-public-debug-'));
const stores=new Set();
const events=`<script>window.clicks=0;window.events=[];for(const type of ['mousemove','mousedown','mouseup','click'])document.addEventListener(type,e=>{events.push({type:e.type,trusted:e.isTrusted,id:e.target.id,x:e.clientX,y:e.clientY});if(e.type==='click')clicks++});</script>`;
const server=http.createServer((req,res)=>{
  const port=server.address().port;res.setHeader('content-type','text/html');
  res.end(req.url==='/parent'?`<!doctype html><iframe style="position:absolute;left:50px;top:60px;width:300px;height:180px;border:0" src="http://localhost:${port}/child"></iframe><iframe style="position:absolute;left:400px;top:60px;width:300px;height:180px;border:0" src="http://localhost:${port}/child"></iframe>`:`<!doctype html><title>Child form</title><style>body{margin:0}button{position:absolute;left:20px;top:20px;width:100px;height:50px}input{position:absolute;left:20px;top:90px}</style><button id="target">Frame button</button><input id="name" aria-label="Name">${events}`);
});
async function launch(args){
  await new Promise((ok,fail)=>{
    const child=spawn(resolve('src-tauri/target/debug/examples/browser_debug'),args,{stdio:['ignore','pipe','inherit']});
    let output='';child.stdout.on('data',chunk=>{output+=chunk;process.stdout.write(chunk)});
    const timeout=setTimeout(()=>child.kill(),60000);
    child.on('error',fail);child.on('close',code=>{
      clearTimeout(timeout);for(const match of output.matchAll(/FIXTURE_STORE:([A-Fa-f0-9-]{36})/g))stores.add(match[1]);
      if(code===0)ok();else fail(Error(`Native adapter fixture exited ${code}`));
    });
  });
}
try{
  await new Promise(ok=>server.listen(0,'127.0.0.1',ok));
  await launch([`http://127.0.0.1:${server.address().port}/parent`,scratch]);
}finally{
  server.closeAllConnections();server.close();
  try{for(const id of stores)await launch(['--remove-store',id]);}
  finally{await rm(scratch,{recursive:true,force:true});}
}
