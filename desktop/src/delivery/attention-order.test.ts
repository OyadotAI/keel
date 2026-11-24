// Durable causes and live requests share one priority order and one row per task alias group.
import {expect,it} from "vitest";
import {orderAttention} from "./attention-order";
import type {AttentionItem} from "../attention";
const live=(lane:string,kind:"input"|"error"|"checks"|"update"):AttentionItem=>({lane,project:"/p",projectName:"P",title:lane,reasons:[{kind,text:kind}],priority:0,action:"Open",destination:{kind:"update"},disconnected:false});
const saved={id:"task",task:"task",title:"Saved task",project:"/p",lanes:["alias"],reasons:["1 unresolved correction"],priority:3,recurring:false};
it("puts saved corrections before unread endings and after inputs/errors/failed checks",()=>{
  const rows=orderAttention([live("update","update"),live("checks","checks"),live("error","error"),live("input","input")],[saved],["/p"]);
  expect(rows.map(row=>row.kind==="live"?row.item.lane:row.item.id)).toEqual(["input","error","checks","task","update"]);
});
it("groups an alias's live error and durable correction without hiding either cause",()=>{
  const rows=orderAttention([live("alias","error")],[saved],["/p"]);expect(rows).toHaveLength(1);
  expect(rows[0].kind).toBe("live");expect(rows[0].item.reasons.map(reason=>typeof reason==="string"?reason:reason.text)).toEqual(["error","1 unresolved correction"]);
});
it("keeps a pending request's exact live destination ahead of saved task navigation",()=>{
  const request=live("alias","input"),rows=orderAttention([request],[saved],["/p"]);
  expect(rows).toHaveLength(1);expect(rows[0].item).toBe(request);
});
