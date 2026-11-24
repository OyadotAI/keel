// Controlled UI feedback preserves the same unresolved and captured-publication semantics as the daemon.
import {expect,it} from "vitest";
import {DeliveryFixture} from "./data";
it("provides the existing task handoff reader its complete empty-history contract",()=>{
  expect(new DeliveryFixture().respond("/api/task","GET",{},new URLSearchParams())).toMatchObject({events:[],task:null,saved:null,checkout_exists:true});
});
it("sending a fixture correction changes delivery history without resolving it",()=>{
  const fixture=new DeliveryFixture(),params=new URLSearchParams();
  fixture.respond("/api/tasks/fixture-task/corrections","POST",{revision:0,body:"Fix the retry",anchor:null},params);
  fixture.respond("/api/tasks/fixture-task/batches","POST",{receipt:"batch",selected:{"correction-0":0}},params);
  expect(fixture.view.record.corrections[0]).toMatchObject({resolution:"unresolved",deliveries:["batch"]});
});
