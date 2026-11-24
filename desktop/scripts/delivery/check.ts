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
window.fetch=async(input,init)=>{
  const target=new URL(String(input),location.href),method=init?.method??"GET";
  if(!target.pathname.startsWith("/api/"))return base(input,init);
  let body={};try{body=JSON.parse(String(init?.body??"{}"));}catch{/* Read-only calls have no body. */}
  try{const data=fixture.respond(target.pathname,method,body,target.searchParams);if(data===undefined)return base(input,init);if(method==="POST")queueMicrotask(()=>deliveryChanged(ep));return Response.json(data);}
  catch(error){return new Response(String(error),{status:409});}
};
const lane=useStore.getState().lanes.update;
useStore.setState((state)=>({lanes:{...state.lanes,update:{...lane,agent:"codex",session:"fixture",known:true,isolated:true,wt:"task-fixture",git:{is_repo:true,branch:fixture.view.record.branch,head:fixture.view.identity?.head,changes:[{path:"src/checkout.ts",status:" M",label:"modified",staged:false,dir:false}]} }},panelTab:"review"}));
void refreshDelivery(ep);
const verify=document.createElement("button");verify.textContent="Verify Delivery controls";Object.assign(verify.style,{position:"fixed",right:"0",bottom:"0",zIndex:"100"});
verify.onclick=()=>{verify.remove();requestAnimationFrame(()=>requestAnimationFrame(()=>{const output=document.createElement("output");output.textContent=verifyDeliveryControls(document);Object.assign(output.style,{position:"fixed",right:"0",bottom:"0",zIndex:"100",background:"var(--raised)"});document.body.append(output);}));};document.body.append(verify);
Object.assign(window,{deliveryFixture:{fixture,useStore,ep,refresh:()=>deliveryChanged(ep),openDiff:()=>{useStore.setState((state)=>({lanes:{...state.lanes,update:{...state.lanes.update,viewing:"src/checkout.ts",diff:prepare(fixture.respond("/api/git/diff","GET",{},new URLSearchParams()) as Parameters<typeof prepare>[0])}}}));useStore.setState({panelTab:"git"});}}});
