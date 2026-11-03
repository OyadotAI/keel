// Interrupted grid moves continue from their displayed boxes and release every animation handle.
import { expect, it, vi } from "vitest";
import { createGridMotion, type Box } from "./grid-motion";

const box = (width: number): Box => ({ left: 0, top: 0, width, height: 600 });
function fixture() {
  let layout = box(800);
  const animations: { cancel: ReturnType<typeof vi.fn>; effect: { getComputedTiming: () => { progress: number } }; onfinish?: () => void }[] = [];
  const pane = { dataset: { pane: "child" }, querySelector: () => null, getBoundingClientRect: () => layout, animate: vi.fn((_frames: Keyframe[], _options: KeyframeAnimationOptions) => {
    const animation = { cancel: vi.fn(), effect: { getComputedTiming: () => ({ progress: .5 }) } };
    animations.push(animation); return animation;
  }) };
  const root = { querySelectorAll: () => [pane], querySelector: () => null } as unknown as HTMLElement;
  return { root, pane, animations, layout: (width: number) => { layout = box(width); } };
}
it("a second split retargets from the current visual width rather than the previous target", () => {
  const f = fixture(), owner = createGridMotion();
  owner.update(f.root, false); f.layout(400); owner.update(f.root, false);
  f.layout(200); owner.update(f.root, false);
  expect(f.animations[0].cancel).toHaveBeenCalledOnce();
  expect(f.pane.animate.mock.calls[1][0]).toEqual([{ transformOrigin: "0 0", transform: "translate(0px, 0px) scale(3, 1)" }, { transformOrigin: "0 0", transform: "none" }]);
  owner.cancel(); expect(f.animations[1].cancel).toHaveBeenCalledOnce();
});
it("main terminal geometry animates without scaling text and releases sizing locks on every exit", () => {
  const f = fixture(), owner = createGridMotion();
  const content = { style: { width: "", height: "", flex: "" }, getBoundingClientRect: () => box(800) };
  const viewport = { dataset: {} as Record<string, string>, animate: f.pane.animate };
  const root = { querySelectorAll: () => [{ ...f.pane, dataset: { pane: "main" }, querySelector: () => viewport }], querySelector: () => content } as unknown as HTMLElement;
  owner.update(root, false); f.layout(400); owner.update(root, false);
  expect(content.style).toEqual({ width: "800px", height: "600px", flex: "none" });
  expect(JSON.stringify(viewport.animate.mock.calls[0][0])).not.toMatch(/scale|filter|opacity/);
  f.layout(200); owner.update(root, false);
  f.animations[0].onfinish!(); expect(content.style.width).toBe("800px");
  f.animations[1].onfinish!(); expect(content.style.width).toBe(""); expect(viewport.dataset.moving).toBeUndefined();
  f.layout(400); owner.update(root, false); owner.update(root, true); expect(content.style.width).toBe("");
  f.layout(600); owner.update(root, false); owner.reset(root); expect(content.style.flex).toBe("");
  f.layout(800); owner.update(root, false); owner.cancel(); expect(content.style.width).toBe("");
});
it("completion, reduced motion and resize reset cannot leave animations competing", () => {
  const f = fixture(), owner = createGridMotion();
  owner.update(f.root, false); f.layout(400); owner.update(f.root, false);
  f.animations[0].onfinish!(); owner.cancel();
  expect(f.animations[0].cancel).toHaveBeenCalledOnce();
  f.layout(200); owner.update(f.root, true);
  expect(f.pane.animate).toHaveBeenCalledOnce();
  f.layout(400); owner.update(f.root, false); owner.reset(f.root); owner.cancel();
  expect(f.animations[1].cancel).toHaveBeenCalledOnce();
});
