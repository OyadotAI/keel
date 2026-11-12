// Pins how a prompt's uploads are found and Keel's trailer is hidden.
import { describe, expect, it } from "vitest";
import { promptAttachments } from "./prompt-attachments";

describe("promptAttachments", () => {
  it("finds the images Keel named and hides its trailer", () => {
    const p = "fix this\n\nAttached files in this checkout (read them):\n.keel/attachments/1-shot.png\n.keel/attachments/2-notes.txt";
    expect(promptAttachments(p)).toEqual({ text: "fix this", images: [".keel/attachments/1-shot.png"] });
  });
  it("hides the trailer Claude and Codex store, and still finds its image", () => {
    const p = "fix this\n\nAttached files in this checkout (already shown to you above):\n.keel/attachments/1-shot.png";
    expect(promptAttachments(p)).toEqual({ text: "fix this", images: [".keel/attachments/1-shot.png"] });
  });
  it("reads an @-mention of an upload too", () => {
    expect(promptAttachments("see @.keel/attachments/a.JPG").images).toEqual([".keel/attachments/a.JPG"]);
  });
  it("refuses a path that climbs out of the uploads", () => {
    expect(promptAttachments(".keel/attachments/../secret.png").images).toEqual([]);
  });
  it("leaves a plain prompt alone", () => {
    expect(promptAttachments("hello")).toEqual({ text: "hello", images: [] });
  });
});
