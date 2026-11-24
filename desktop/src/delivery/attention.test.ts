// Task-based attention survives closed lanes and keeps handoff aliases on a single persisted row.
import {expect,it} from "vitest";
import {savedAttention} from "./attention";
import type {TaskSummary} from "./model";
const task:TaskSummary={task:"stable-task",title:"Fix checkout",source:"owner/repo#1",owner:{lane:"current",provider:"codex",session:"s",checkout:"/p"},lanes:["old","current"],unresolved:2,remote_open:1,managed:true};
it("keys corrections by task instead of duplicating provider handoff lanes",()=>{
  const items=savedAttention({tasks:[task]});expect(items).toHaveLength(1);expect(items[0]).toMatchObject({id:"stable-task",lanes:["old","current"],reasons:["2 unresolved corrections"]});
});
it("keeps failed historical checks and uncertain recurring slots visible",()=>{
  const items=savedAttention({tasks:[{...task,unresolved:0,check:{id:"check",status:"failed",source:"turn",command:null,plan:"plan",started_at:1,completed_at:2,before:null,after:null,successor:null,problems:[]}}]}, {schedules:[{id:"s",revision:0,name:"Maintenance",prompt:"check",provider:"codex",base:"main",interval:60,duration:60,enabled:false}],active:"run",occurrences:[{id:"run",schedule:"s",phase:"unconfirmed",due_at:1}]});
  expect(items.map((item)=>item.reasons.join())).toEqual(["Last recorded checks: failed","Active run: unconfirmed"]);
});
