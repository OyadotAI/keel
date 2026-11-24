// Named synthetic failure states are reachable by URL through Oya without a console evaluator.
import type { DeliveryFixture } from "./data";
const HISTORY_COUNT=120, INTERVAL_SECONDS=60;
/** Fixtures change controlled API state only; the real App still owns every interaction. */
export function seedScenario(fixture:DeliveryFixture,scenario:string|null) {
  const states:Record<string,()=>void>={
    controller:()=>{seedScenario(fixture,"active");fixture.controllerError="Execution history is full. Existing records were preserved.";},
    uncertain:()=>{fixture.failNext=true;},
    stale:()=>{fixture.view.checks_current=false;fixture.view.checks_passed=false;fixture.view.identity!.tree="c".repeat(40);},
    active:()=>{
      fixture.schedules=[{id:"daily",revision:1,name:"Daily audit",prompt:"Inspect checks",provider:"codex",base:"main",interval:INTERVAL_SECONDS,duration:INTERVAL_SECONDS,enabled:true}];
      fixture.occurrences=Array.from({length:HISTORY_COUNT},(_,index)=>({id:`run-${index}`,schedule:"daily",phase:index===HISTORY_COUNT-1?"unconfirmed":"completed",due_at:HISTORY_COUNT-index,task:"fixture-task"}));
      fixture.active=`run-${HISTORY_COUNT-1}`;
    },
  };
  states[scenario??""]?.();
}
