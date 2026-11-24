// The real App uses controlled stateful delivery APIs while preserving its actual layout and terminal mounts.
import "../attention/check";
import { DeliveryFixture } from "./data";
import { seedScenario } from "./scenarios";
import {verifyDeliveryControls} from "./layout";
import { useStore } from "../../src/store";
import { deliveryChanged,refreshDelivery } from "../../src/delivery";
import { prepare } from "../../src/git";
import "../../src/delivery.css";

const fixture=new DeliveryFixture(), base=window.fetch.bind(window), ep={port:Number(location.port),token:"synthetic-fixture"};
seedScenario(fixture,new URLSearchParams(location.search).get("scenario"));
// `?open=tasks` opens the Tasks dialog on load; `&many=1` lists the kinds of titles real use produces.
const params=new URLSearchParams(location.search), many=params.has("many");
function manyTasks(page:{tasks:unknown[]}){
  const base=page.tasks[0] as Record<string,unknown>&{owner:Record<string,unknown>};
  const row=(task:string,title:string,provider:string,extra:Record<string,unknown>={})=>({...base,task,title,source:"Conversation task",owner:{...base.owner,provider},unresolved:0,check:null,publication:null,...extra});
  return {...page,tasks:[...page.tasks,
    row("t1","there are alot of work from codex but its not finish and codex hit the wall... alot of people like https://github.com/stablyai/orca dive deep and add the missing features","claude",{unresolved:3}),
    row("t2","https://github.com/jumper-media/jm-local-v2/actions/runs/42022447662/job/127296469666?pr=452","codex",{publication:{url:"https://github.com/owner/repo/pull/452"}}),
    row("t3","i need landing page for this","codex"),
    row("t4","Fix: a closed Oya tab locks the lane's browser until restart (\"Tab N not found\")","codex"),
    row("t5","hi","claude"),
    row("t6","Bug: Keel's Browser tab can't test a mobile layout, and browser_read misses frames","antigravity"),
  ]};
}
if(params.get("open")==="start")setTimeout(()=>useStore.setState({starting:useStore.getState().order[0]}),500);
if(params.get("open")==="tasks"){const tryOpen=(left:number)=>{const b=[...document.querySelectorAll<HTMLButtonElement>("button.attention-entry")].find((x)=>x.textContent?.includes("Tasks"));if(b&&!document.querySelector(".delivery-dialog"))b.click();else if(left>0)setTimeout(()=>tryOpen(left-1),250);};setTimeout(()=>tryOpen(20),500);}
window.fetch=async(input,init)=>{
  const target=new URL(String(input),location.href),method=init?.method??"GET";
  if(!target.pathname.startsWith("/api/"))return base(input,init);
  let body={};try{body=JSON.parse(String(init?.body??"{}"));}catch{/* Read-only calls have no body. */}
  try{const data=target.pathname==="/api/tasks"&&many?manyTasks(fixture.respond(target.pathname,method,body,target.searchParams) as {tasks:unknown[]}):fixture.respond(target.pathname,method,body,target.searchParams);if(data===undefined)return base(input,init);if(method==="POST")queueMicrotask(()=>deliveryChanged(ep));return Response.json(data);}
  catch(error){return new Response(String(error),{status:409});}
};
const lane=useStore.getState().lanes.update;
useStore.setState((state)=>({lanes:{...state.lanes,update:{...lane,agent:"codex",session:"fixture",known:true,isolated:true,wt:"task-fixture",git:{is_repo:true,branch:fixture.view.record.branch,head:fixture.view.identity?.head,changes:[{path:"src/checkout.ts",status:" M",label:"modified",staged:false,dir:false}]} }},panelTab:"review"}));
void refreshDelivery(ep);
const verify=document.createElement("button");verify.textContent="Verify Delivery controls";Object.assign(verify.style,{position:"fixed",right:"0",bottom:"0",zIndex:"100"});
verify.onclick=()=>{verify.remove();requestAnimationFrame(()=>requestAnimationFrame(()=>{const output=document.createElement("output");output.textContent=verifyDeliveryControls(document);Object.assign(output.style,{position:"fixed",right:"0",bottom:"0",zIndex:"100",background:"var(--raised)"});document.body.append(output);}));};document.body.append(verify);
Object.assign(window,{deliveryFixture:{fixture,useStore,ep,refresh:()=>deliveryChanged(ep),openDiff:()=>{useStore.setState((state)=>({lanes:{...state.lanes,update:{...state.lanes.update,viewing:"src/checkout.ts",diff:prepare(fixture.respond("/api/git/diff","GET",{},new URLSearchParams()) as Parameters<typeof prepare>[0])}}}));useStore.setState({panelTab:"git"});}}});
