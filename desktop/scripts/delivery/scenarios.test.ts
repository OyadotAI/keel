// Failure variants retain truthful fixture contracts for browser recovery checks.
import {expect,it} from "vitest";
import {DeliveryFixture} from "./data";
import {seedScenario} from "./scenarios";
it("returns active recovery independently of the bounded history",()=>{
  const fixture=new DeliveryFixture();seedScenario(fixture,"active");
  const page=fixture.respond("/api/delivery/schedules","GET",{},new URLSearchParams()) as {occurrences:{id:string}[];active_occurrence:{id:string;phase:string}};
  expect(page.occurrences).toHaveLength(100);expect(page.active_occurrence).toMatchObject({id:"run-119",phase:"unconfirmed"});expect(page.occurrences.some(o=>o.id===page.active_occurrence.id)).toBe(false);
});
it("can exercise stale evidence and a lost publication response without live services",()=>{
  const fixture=new DeliveryFixture();seedScenario(fixture,"stale");expect(fixture.view.checks_passed).toBe(false);seedScenario(fixture,"uncertain");expect(fixture.failNext).toBe(true);
});
