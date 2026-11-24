// Active recovery must remain visible even when a bounded history page excludes the active run.
import { expect,it,vi } from "vitest";
vi.mock("../store",()=>({AGENT_NAME:{codex:"Codex"}}));
vi.mock("../delivery",()=>({deliveryChanged:vi.fn(),requestId:vi.fn(),forgetRequest:vi.fn()}));
import { renderToStaticMarkup } from "react-dom/server";
import { RecurringTasks } from "./TaskSchedules";
import type { SchedulePage } from "../delivery";
it("renders Stop and confirmation from the independently returned active occurrence",()=>{
  const page={schedules:[{id:"schedule",name:"Daily",enabled:true,interval:60,duration:60}],occurrences:[],active:"old",active_occurrence:{id:"old",schedule:"schedule",phase:"unconfirmed",due_at:1,reason:"Inspect original task"},next:"100:cursor"} as unknown as SchedulePage;
  const html=renderToStaticMarkup(<RecurringTasks ep={{port:123,token:"test"}} page={page} open={async()=>{}}/>);
  expect(html).toContain("Stop active run");expect(html).toContain("Confirm original run stopped");expect(html).toContain("More occurrence history");
});
it("shows a dispatcher halt and explicit Retry while retaining active Stop",()=>{
  const page={controller_error:"Execution history is full",schedules:[{id:"s",name:"Audit",enabled:true,interval:60,duration:60}],occurrences:[],active:"active",active_occurrence:{id:"active",schedule:"s",phase:"unconfirmed",due_at:1}} as unknown as SchedulePage;
  const html=renderToStaticMarkup(<RecurringTasks ep={{port:123,token:"test"}} page={page} open={async()=>{}}/>);
  expect(html).toContain("Recurring dispatch paused");expect(html).toContain("Retry recurring dispatcher");expect(html).toContain("Stop active run");expect(html).toContain("Dispatch paused");
});
it("states a cadence in the largest unit that divides it",async()=>{
  const { every } = await import("./TaskSchedules");
  expect(every(86400)).toBe("1 day");expect(every(6*3600)).toBe("6 hours");expect(every(45*60)).toBe("45 minutes");expect(every(60)).toBe("1 minute");
});
