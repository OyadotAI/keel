// Stateful controlled APIs exercise actual task, correction and publication UI request contracts.
import type { TaskView,Receipt,Schedule,Occurrence } from "../../src/delivery";
export const HEAD = "a".repeat(40), TREE = "b".repeat(40);
const HISTORY_LIMIT=100;
const owner = {lane:"update",provider:"codex" as const,session:"fixture",checkout:"/fixture/shop/.keel/worktrees/task-fixture",wt:"task-fixture"};
const form = {title:"Preserve checkout receipt identity",body:"Source: https://github.com/owner/repo/issues/42\n\nProject checks: passed (current)\nCommand: pnpm test\n",draft:true,base:"main",edited:false,seeded:true};
/** The exact same task stays recoverable after its window lane closes or changes provider. */
export function initialTask():TaskView {return {record:{task:"fixture-task",revision:0,title:"Preserve checkout receipt identity",source:{kind:"issue",snapshot:{title:"Preserve checkout receipt identity",url:"https://github.com/owner/repo/issues/42",body:"Retry a checkout without making another payment.\n".repeat(8),repository:"owner/repo",number:42,fetched_at:1}},owner,base:"main",branch:"keel/task-fixture",checks:[{id:"check-1",status:"passed",source:"turn:fixture",command:"pnpm test",plan:"test",started_at:1,completed_at:2,before:{head:HEAD,tree:TREE},after:{head:HEAD,tree:TREE},successor:null,problems:[]}],reviews:[],corrections:[],form},owner,identity:{head:HEAD,tree:TREE},checks_current:true,checks_passed:true,operations:[]};}
export class DeliveryFixture {
  view=initialTask();receipts=new Map<string,Receipt>();schedules:Schedule[]=[];occurrences:Occurrence[]=[];active:string|null=null;controllerError:string|null=null;failNext=false;calls:{path:string;body:unknown}[]=[];
  /** Mutations expose the same optimistic revision conflicts as the real daemon. */
  respond(path:string,method:string,body:Record<string,unknown>,params:URLSearchParams):unknown {
    this.calls.push({path,body});
    const direct=this.routes(body,method,params)[path];
    if(direct)return direct();
    if(path.startsWith("/api/delivery/receipts/"))return this.receipts.get(path.split("/").at(-1)!);
    if(path.startsWith("/api/delivery/schedules/"))return this.scheduleAction(path,body);
    if(path.startsWith("/api/tasks/fixture-task/"))return this.taskAction(path,body);
    if(path==="/api/git/status")return {is_repo:true,branch:this.view.record.branch,head:this.view.identity?.head,changes:[{path:"src/checkout.ts",status:" M",label:"modified",staged:false,dir:false}]};
    if(path==="/api/git/diff")return {path:"src/checkout.ts",identity:this.view.identity,hunks:[{header:"@@ -1,3 +1,4 @@",lines:[{kind:"ctx",old:1,new:1,text:"export function checkout(key: string) {"},{kind:"del",old:2,new:null,text:"  return submit(key);"},{kind:"add",old:null,new:2,text:"  return submitOnce(key);"},{kind:"add",old:null,new:3,text:"  // preserve the original receipt"},{kind:"ctx",old:3,new:4,text:"}"}]}]};
    return undefined;
  }
  private routes(body:Record<string,unknown>,method:string,params:URLSearchParams):Record<string,()=>unknown>{return {
    "/api/tasks":()=>({tasks:[{...this.view.record,owner:this.view.owner,source:"owner/repo#42",managed:true,unresolved:this.view.record.corrections.filter((c)=>c.resolution==="unresolved").length,remote_open:0,check:this.view.record.checks.at(-1),lanes:["old-owner","update"]}],next:null}),
    "/api/delivery/by-lane":()=>params.get("lane")==="update"?this.view:null,
    "/api/tasks/fixture-task":()=>this.view,
    "/api/tasks/fixture-task/recover":()=>this.view.owner,
    "/api/task":()=>({task:null,saved:null,events:[],truncated:false,current_branch:this.view.record.branch,current_head:this.view.identity?.head,checkout_exists:true,ownership_error:null}),
    "/api/prereview":()=>({review:null,head:this.view.identity?.head,target:{branch:this.view.record.branch,base:this.view.record.base,head:this.view.identity?.head}}),
    "/api/git/branches":()=>({current:this.view.record.branch,local:[{name:this.view.record.branch,current:true,ahead:1,behind:0}],remote:[],remote_details:[],remotes:[],staged:0,unstaged:1}),
    "/api/delivery/issues/preview":()=>({id:"preview-42",issue:{title:form.title,body:this.view.record.source.snapshot!.body,url:"https://github.com/owner/repo/issues/42",state:"open",truncated:false},change_repository:"owner/repo"}),
    "/api/delivery/tasks/start":()=>this.saveReceipt(String(body.receipt),"issue","completed"),
    "/api/delivery/schedules":()=>this.scheduleList(body,method),
  };}
  private scheduleList(body:Record<string,unknown>,method:string){
    if(method!=="POST")return {schedules:this.schedules,occurrences:this.occurrences.slice(0,HISTORY_LIMIT),active:this.active,active_occurrence:this.occurrences.find((o)=>o.id===this.active)??null,controller_error:this.controllerError};
    const schedule={...body,id:body.id as string??`schedule-${this.schedules.length}`,revision:0,enabled:false} as unknown as Schedule;this.schedules.push(schedule);return schedule;
  }
  private scheduleAction(path:string,body:Record<string,unknown>){
    if(body.action==="retry"){this.controllerError=null;return {recovered:true};}
    const schedule=this.schedules.find((s)=>s.id===path.split("/").at(-1))!;
    const actions:Record<string,()=>void>={enable:()=>{schedule.enabled=true;schedule.revision++;schedule.next_at=Date.now()+schedule.interval*1000;},pause:()=>{schedule.enabled=false;schedule.revision++;},stop:()=>{const active=this.occurrences.find((o)=>o.id===this.active);if(active)active.phase="stopping";},confirm_stopped:()=>{const active=this.occurrences.find((o)=>o.id===this.active);if(active)active.phase="stopped";this.active=null;},run_now:()=>{if(!this.occurrences.some((o)=>o.id===body.receipt))this.occurrences.unshift({id:String(body.receipt),schedule:schedule.id,task:"fixture-task",phase:"completed",due_at:Date.now()});}};
    actions[String(body.action)]?.();return schedule;
  }
  private saveReceipt(id:string,kind:string,phase:string){const receipt:Receipt={id,kind,phase,task:"fixture-task",result:{owner:this.view.owner!,task:"fixture-task"}};this.receipts.set(id,receipt);this.view.operations=[receipt,...(this.view.operations??[]).filter((r)=>r.id!==id)];return receipt;}
  private taskAction(path:string,body:Record<string,unknown>):unknown {
    const record=this.view.record;
    if(body.revision!==undefined &&body.revision!==record.revision)throw new Error("Task changed. Refresh its current revision; your draft was retained.");
    if(path.endsWith("/corrections")){const correction={id:`correction-${record.corrections.length}`,revision:0,source:"local",body:String(body.body),anchor:body.anchor as never,resolution:"unresolved" as const,outdated:false,deliveries:[]};record.corrections.push(correction);record.revision++;return correction;}
    if(path.includes("/corrections/")){const correction=record.corrections.find((c)=>c.id===decodeURIComponent(path.split("/").at(-1)!))!;if(body.body!==null)correction.body=String(body.body);if(body.resolved!==null)correction.resolution=body.resolved?"human_resolved":"unresolved";correction.revision++;record.revision++;return correction;}
    if(path.endsWith("/batches")){for(const correction of record.corrections)if((body.selected as object) &&Object.hasOwn(body.selected as object,correction.id))correction.deliveries.push(String(body.receipt));record.revision++;return this.saveReceipt(String(body.receipt),"batch","completed");}
    if(path.endsWith("/checks")){this.view.checks_current=true;this.view.checks_passed=true;return this.saveReceipt(String(body.receipt),"check","completed");}
    if(path.endsWith("/form/seed"))return record.form;
    if(path.endsWith("/form")){record.form=body.form as typeof form;record.revision++;return record.form;}
    if(path.endsWith("/publication")){const receipt=this.saveReceipt(String(body.receipt),"publication",this.failNext?"unconfirmed":"completed");receipt.target={head:this.view.identity!.head,branch:record.branch,base:record.base,repository:"owner/repo"};record.publication={receipt:receipt.id,intent:{identity:this.view.identity!,branch:record.branch,base:record.base,repository:"owner/repo"},status:this.failNext?"unconfirmed":"confirmed",url:this.failNext?null:"https://github.com/owner/repo/pull/7",actual_head:this.failNext?null:this.view.identity!.head};this.failNext=false;return receipt;}
    if(path.endsWith("/retry")){const receipt=this.receipts.get(path.split("/").at(-2)!)!;receipt.phase="completed";if(record.publication){record.publication.status="confirmed";record.publication.url="https://github.com/owner/repo/pull/7";record.publication.actual_head=record.publication.intent.identity.head;}return receipt;}
    if(path.endsWith("/feedback")){record.feedback={head:HEAD,fetched_at:Date.now(),complete:true,errors:[]};return record.feedback;}
    return {};
  }
}
