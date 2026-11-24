// Correction text owns bracket keys and Escape cancels its innermost editor before navigation.
import { expect,it } from "vitest";
import { diffCommand } from "./diff-key";
const keys={metaKey:false,ctrlKey:false,altKey:false,editable:false,correcting:false};
it("leaves bracketed code in a correction field",()=>{for(const key of ["[","]"])expect(diffCommand({...keys,key,editable:true,correcting:true})).toBeUndefined();});
it("retains file navigation outside editable fields",()=>{expect(diffCommand({...keys,key:"["})).toBe("previous");expect(diffCommand({...keys,key:"]"})).toBe("next");});
it("cancels the innermost editor before closing the diff",()=>{expect(diffCommand({...keys,key:"Escape",editable:true,correcting:true})).toBe("cancel");expect(diffCommand({...keys,key:"Escape"})).toBe("back");});
