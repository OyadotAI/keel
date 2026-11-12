// Pins a drop on a chat lane: each file is read and uploaded on its own, and one failure is said.
import { describe, expect, it, vi } from "vitest";
const store = vi.hoisted(() => ({ state: { lanes: { l: {} as { error?: string } } } }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../typers", () => ({ pasteInto: vi.fn() }));
vi.mock("./Composer", () => ({ uploadFiles: vi.fn() }));
vi.mock("../store", () => ({ useStore: { getState: () => store.state, setState: (f: (s: typeof store.state) => typeof store.state) => { store.state = f(store.state); } } }));
import { attachDropped } from "./Drop";

describe("a drop on a chat lane", () => {
  it("uploads each file as a pasted one would be, with its name and type", async () => {
    const sent: File[] = [];
    await attachDropped("l", ["/Users/me/Desktop/shot one.PNG", "C:\\Users\\me\\notes.txt"], async () => new ArrayBuffer(3), async (_lane, files) => { sent.push(...files); });
    expect(sent.map((f) => [f.name, f.type, f.size])).toEqual([["shot one.PNG", "image/png", 3], ["notes.txt", "", 3]]);
  });
  it("says why one file failed and still attaches the others", async () => {
    const sent: string[] = [];
    const read = async (path: string) => { if (path.endsWith("docs")) throw "Folders can't be attached. Drop the files inside, or type the folder's path."; return new ArrayBuffer(1); };
    await attachDropped("l", ["/a/docs", "/a/b.png"], read, async (_lane, files) => { sent.push(files[0].name); });
    expect(sent).toEqual(["b.png"]);
    expect(store.state.lanes.l.error).toBe("Folders can't be attached. Drop the files inside, or type the folder's path.");
    await attachDropped("l", ["/a/c.png"], async () => new ArrayBuffer(1), async () => { throw new Error("disk full"); });
    expect(store.state.lanes.l.error).toBe("Could not attach c.png: disk full");
  });
});
