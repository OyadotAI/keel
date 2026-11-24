// A delayed task read cannot accept text under a pending identity and erase it when ownership arrives.
import {expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import type {TaskView} from "../delivery/model";
const state=vi.hoisted(()=>({view:undefined as TaskView|undefined}));
vi.mock("../store",()=>({useStore:(select:(s:unknown)=>unknown)=>select({lanes:{lane:{project:"/project"}},projects:{"/project":{endpoint:{port:1,token:"test"}}}})}));
vi.mock("../delivery",async()=>({useDelivery:()=>({view:state.view}),mutateTask:vi.fn(),...(await vi.importActual("../delivery/correction-drafts"))}));
import {DiffCorrection} from "./DiffCorrection";
import {initialTask} from "../../scripts/delivery/data";
import type {Prepared} from "../git";
it("blocks draft input while the task read is pending and enables it only under the resolved task",()=>{
  const diff={identity:{head:"a",tree:"b"},rows:[]} as unknown as Prepared;
  const render=()=>renderToStaticMarkup(<DiffCorrection lane="lane" path="file.ts" diff={diff} setRange={()=>{}} close={()=>{}}/>);
  state.view=undefined;expect(render()).toMatch(/<textarea[^>]*disabled=""/);
  state.view=initialTask();expect(render()).not.toMatch(/<textarea[^>]*disabled=""/);
});
