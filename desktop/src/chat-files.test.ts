// Messages that will not be sent go back into the composer rather than disappearing.
import { describe, expect, it } from "vitest";
import { backToComposer } from "./chat-files";
import type { Lane } from "./store";

const lane = (draft: string, attachments: Lane["attachments"] = []) => ({ draft, attachments, references: [] }) as unknown as Lane;

describe("backToComposer", () => {
  it("puts stopped queue messages after what is being typed, attachments included", () => {
    const a = { path: "a.png", name: "a.png" };
    const patch = backToComposer(lane("typing"), [{ text: "first", attachments: [a] }, { text: "second" }]);
    expect(patch.draft).toBe("typing\n\nfirst\n\nsecond");
    expect(patch.attachments).toEqual([a]);
  });
  it("changes nothing when nothing was unsent", () => {
    expect(backToComposer(lane("typing"), [])).toEqual({});
  });
  it("does not start the draft with a blank paragraph", () => {
    expect(backToComposer(lane(""), [{ text: "only" }]).draft).toBe("only");
  });
});
