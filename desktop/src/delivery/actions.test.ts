// Receipt identity survives reload and separates changed intent from a confirmed repeatable action.
import { beforeEach,afterEach,expect,it,vi } from "vitest";
const saved = new Map<string,string>();
beforeEach(() => { vi.resetModules(); saved.clear(); vi.stubGlobal("localStorage", { getItem: (key:string) => saved.get(key) ?? null, setItem: (key:string,value:string) => saved.set(key,value) }); });
afterEach(() => vi.unstubAllGlobals());
it("reuses a saved receipt through reload and assigns changed corrections another receipt",async () => {
  let actions = await import("./actions");
  const first = actions.requestId("batch:task",{ selected: { a: 1 } });
  vi.resetModules(); actions = await import("./actions");
  expect(actions.requestId("batch:task",{ selected: { a: 1 } })).toBe(first);
  expect(actions.requestId("batch:task",{ selected: { a: 2 } })).not.toBe(first);
});
it("only a confirmed repeatable action obtains a fresh receipt on its next run",async () => {
  const actions = await import("./actions"), input = { revision: 3 };
  const receipt = actions.requestId("schedule:one",input);
  expect(actions.requestId("schedule:one",input)).toBe(receipt);
  actions.forgetRequest(receipt); expect(actions.requestId("schedule:one",input)).not.toBe(receipt);
});
it("opens a saved owner with the router's read-only recovery method",async()=>{
  const fetch=vi.fn().mockResolvedValue({ok:true,text:async()=>'{"lane":"owner"}'});vi.stubGlobal("fetch",fetch);
  const actions=await import("./actions");expect(await actions.recoverOwner({port:12,token:"test"},"task-id")).toEqual({lane:"owner"});
  expect(fetch.mock.calls[0][0]).toBe("http://127.0.0.1:12/api/tasks/task-id/recover");expect(fetch.mock.calls[0][1].method).toBe("GET");
});
