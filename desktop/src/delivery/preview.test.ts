// Superseded URL reads and failures cannot replace the person's current issue choice.
import { expect,it } from "vitest";
import { IssuePreviewReader } from "./preview";
import type { IssuePreview } from "./model";
it("invalidates a pending issue when the person edits its URL",async()=>{
  let finish!:(preview:IssuePreview)=>void;
  const reader=new IssuePreviewReader(()=>new Promise(resolve=>{finish=resolve;}));
  const pending=reader.read("issue-a");reader.invalidate();finish({id:"a"} as IssuePreview);
  expect(await pending).toBeUndefined();
});
it("discards a stale failure without clearing a newer successful preview",async()=>{
  let reject!:(error:Error)=>void;
  const reader=new IssuePreviewReader(url=>url==="a"?new Promise((_,fail)=>{reject=fail;}):Promise.resolve({id:"b"} as IssuePreview));
  const pending=reader.read("a");expect(await reader.read("b")).toEqual({preview:{id:"b"}});
  reject(new Error("old error"));expect(await pending).toBeUndefined();
});
