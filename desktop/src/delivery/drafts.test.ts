// PR body and title edits survive review reads, provider handoff and window restart.
import { beforeEach,afterEach,expect,it,vi } from "vitest";
import type { PrForm } from "./model";
const saved = new Map<string,string>();
const form:PrForm = { title:"Issue title",body:"Source and actual checks",draft:true,base:"main",seeded:true,edited:false };
beforeEach(() => {vi.resetModules();saved.clear();vi.stubGlobal("localStorage",{getItem:(key:string)=>saved.get(key)??null,setItem:(key:string,value:string)=>saved.set(key,value)});});
afterEach(() => vi.unstubAllGlobals());
it("preserves dirty title body and draft over server reads and a module restart",async()=>{
  let drafts=await import("./drafts");const edit={...form,title:"Person's title",body:"Person's exact body",draft:false};
  drafts.editPrDraft("stable-task","/same-checkout",edit);
  expect(drafts.prDraft("stable-task","/same-checkout",form)).toMatchObject({form:{title:edit.title,body:edit.body,draft:false},dirty:true});
  vi.resetModules();drafts=await import("./drafts");expect(drafts.prDraft("stable-task","/same-checkout",form).form.body).toBe(edit.body);
});
it("a late save confirmation cannot erase a newer body edit",async()=>{
  const drafts=await import("./drafts"), first={...form,edited:true}, later={...first,body:"newer keystrokes"};
  drafts.editPrDraft("task","/checkout",first);drafts.editPrDraft("task","/checkout",later);drafts.savedPrDraft("task","/checkout",first);
  expect(drafts.prDraft("task","/checkout",form)).toMatchObject({dirty:true,form:{body:later.body}});
});
