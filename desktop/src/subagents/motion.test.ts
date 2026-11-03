// Reveals dissolve from their current appearance and never leave permanent layers or handles.
import { afterEach, expect, it, vi } from "vitest";
import { createPaneMotion } from "./motion";
afterEach(() => vi.unstubAllGlobals());
function fixture() {
  const animations: { cancel: ReturnType<typeof vi.fn>; onfinish?: () => void }[] = [];
  const node = { dataset: {} as Record<string, string>, style: { opacity: "", removeProperty: vi.fn() }, animate: vi.fn((_frames: Keyframe[], _options: KeyframeAnimationOptions) => {
    const animation = { cancel: vi.fn() }; animations.push(animation); return animation;
  }) };
  node.style.removeProperty.mockImplementation(() => { node.style.opacity = ""; });
  vi.stubGlobal("getComputedStyle", () => ({ opacity: ".4", transform: "matrix(1, 0, 0, 1, 0, 3)" }));
  return { node, element: node as unknown as HTMLElement, animations };
}
it("closing during a reveal starts at the displayed opacity and translation without brightening", () => {
  const f = fixture(), owner = createPaneMotion();
  owner.play(f.element, false, false); owner.play(f.element, true, false);
  expect(f.animations[0].cancel).toHaveBeenCalledOnce();
  expect(f.node.animate.mock.calls[1][0][0]).toMatchObject({ opacity: ".4", transform: "matrix(1, 0, 0, 1, 0, 3)" });
  f.animations[0].onfinish!(); expect(f.node.dataset.motion).toBe("closing");
  f.animations[1].onfinish!(); expect(f.node.dataset.motion).toBeUndefined(); expect(f.node.style.opacity).toBe("0");
  owner.cancel(); expect(f.node.style.opacity).toBe("");
});
it("reduced motion cancels active effects and re-enabling it does not replay a settled reveal", () => {
  const f = fixture(), owner = createPaneMotion();
  owner.play(f.element, false, false); owner.play(f.element, false, true);
  expect(f.animations[0].cancel).toHaveBeenCalledOnce(); expect(f.node.dataset.motion).toBeUndefined();
  owner.play(f.element, false, false); expect(f.node.animate).toHaveBeenCalledOnce();
});
