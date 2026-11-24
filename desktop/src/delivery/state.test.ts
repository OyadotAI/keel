// Endpoint generations discard stale reads and partial failures preserve the last useful task list.
import { afterEach,expect,it,vi } from "vitest";
const api=vi.hoisted(()=>({get:vi.fn()}));vi.mock("../api",()=>({get:api.get}));
afterEach(()=>{vi.resetModules();api.get.mockReset();});
const ep={port:12,token:"window"};
it("a failed refresh revokes current evidence while preserving cached history",async()=>{
  const state=await import("./state");const {initialTask}=await import("../../scripts/delivery/data");const view=initialTask();
  expect(state.observedTask({view,error:"offline"})).toMatchObject({checks_current:false,checks_passed:false,control_error:"offline",record:{task:"fixture-task"}});
  expect(view.checks_passed).toBe(true);
});
it("a late project read cannot replace a newer discovery snapshot",async()=>{
  let finish!:(value:unknown)=>void;
  api.get.mockImplementation((_ep,path)=>path==="/api/tasks"?new Promise((resolve)=>{finish=resolve;}):Promise.resolve({schedules:[],occurrences:[]}));
  const state=await import("./state"), old=state.refreshDelivery(ep);
  api.get.mockImplementation((_ep,path)=>Promise.resolve(path==="/api/tasks"?{tasks:[{task:"new"}]}:{schedules:[],occurrences:[]}));
  await state.refreshDelivery(ep);finish({tasks:[{task:"old"}]});await old;
  expect(state.projectDelivery(ep)?.tasks?.tasks[0].task).toBe("new");
});
it("execution read failure keeps previously observed task discovery with a visible error",async()=>{
  const state=await import("./state");api.get.mockImplementation((_ep,path)=>Promise.resolve(path==="/api/tasks"?{tasks:[{task:"saved"}]}:{schedules:[],occurrences:[]}));await state.refreshDelivery(ep);
  api.get.mockRejectedValue(new Error("storage unavailable"));await state.refreshDelivery(ep);
  expect(state.projectDelivery(ep)?.tasks?.tasks[0].task).toBe("saved");expect(state.projectDelivery(ep)?.error).toContain("storage unavailable");
});
