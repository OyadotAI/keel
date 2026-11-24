// Cancelled drafts retain text while confirmed saves and distinct tasks remain independent.
import {expect,it} from "vitest";
import {correctionDraft,editCorrectionDraft} from "./correction-drafts";
it("retains a cancelled correction for its task and file through another read",()=>{editCorrectionDraft("task:a:file:x","Use values[index]");expect(correctionDraft("task:a:file:x")).toBe("Use values[index]");expect(correctionDraft("task:b:file:x")).toBe("");});
it("removes only the saved draft and restores the daemon's current text",()=>{editCorrectionDraft("edit:a","New text");editCorrectionDraft("edit:b","Keep me");editCorrectionDraft("edit:a");expect(correctionDraft("edit:a","Saved text")).toBe("Saved text");expect(correctionDraft("edit:b")).toBe("Keep me");});
