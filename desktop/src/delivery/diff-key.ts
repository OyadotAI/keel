// Diff shortcuts leave editable fields alone while Escape cancels the innermost edit first.
interface Keys {key:string;metaKey:boolean;ctrlKey:boolean;altKey:boolean;editable:boolean;correcting:boolean}
type Command = "back"|"cancel"|"previous"|"next";
const COMMANDS:Record<string,Command|undefined>={"[":"previous","]":"next"};
/** Bracketed code in a correction is text, so it can never navigate or discard the form. */
export function diffCommand(event:Keys):Command|undefined {
  if(event.metaKey||event.ctrlKey||event.altKey)return;
  if(event.key==="Escape")return event.correcting?"cancel":"back";
  return event.editable?undefined:COMMANDS[event.key];
}
