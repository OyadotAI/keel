// Window-scoped correction drafts survive cancellation and panel remounts under their original task.
import { useSyncExternalStore } from "react";
const drafts=new Map<string,string>(),listeners=new Set<()=>void>();
/** Different tasks, files and edit purposes cannot inherit one another's unsaved prose. */
export function correctionDraft(scope:string,initial="") {return drafts.get(scope)??initial;}
/** Clearing happens only after a confirmed save; cancelling retains the draft. */
export function editCorrectionDraft(scope:string,body?:string) {if(body===undefined)drafts.delete(scope);else drafts.set(scope,body);listeners.forEach(listener=>listener());}
/** The live cache restores text without coupling draft ownership to a transient component. */
export function useCorrectionDraft(scope:string,initial=""):readonly[string,(body:string)=>void,()=>void] {
  const snapshot=()=>correctionDraft(scope,initial);
  const body=useSyncExternalStore(listener=>{listeners.add(listener);return()=>{listeners.delete(listener);};},snapshot,snapshot);
  return [body,(value)=>editCorrectionDraft(scope,value),()=>{if(correctionDraft(scope,initial)===body)editCorrectionDraft(scope);}];
}
