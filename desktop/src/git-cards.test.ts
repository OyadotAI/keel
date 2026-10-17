/** Card preferences survive panel remounts without making storage a requirement. */
import { afterEach, expect, it, vi } from "vitest";

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });

it("starts with both cards open", async () => {
  vi.stubGlobal("localStorage", { getItem: () => null });
  const { useGitCards } = await import("./git-cards");
  expect(useGitCards.getState()).toMatchObject({ changes: true, graph: true });
});

it("closes and reopens cards independently and restores them on reload", async () => {
  const saved = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (key: string) => saved.get(key), setItem: (key: string, value: string) => saved.set(key, value) });
  const { useGitCards } = await import("./git-cards");
  useGitCards.getState().setOpen("graph", false);
  expect(useGitCards.getState()).toMatchObject({ changes: true, graph: false });
  useGitCards.getState().setOpen("changes", false);
  vi.resetModules();
  const restored = (await import("./git-cards")).useGitCards;
  expect(restored.getState()).toMatchObject({ changes: false, graph: false });
  restored.getState().setOpen("changes", true);
  restored.getState().setOpen("graph", true);
  expect(restored.getState()).toMatchObject({ changes: true, graph: true });
  expect([...saved.values()]).toEqual(["open", "open"]);
});

it("still toggles when storage is unavailable", async () => {
  vi.stubGlobal("localStorage", { getItem() { throw Error("blocked"); }, setItem() { throw Error("blocked"); } });
  const { useGitCards } = await import("./git-cards");
  expect(useGitCards.getState().graph).toBe(true);
  useGitCards.getState().setOpen("graph", false);
  expect(useGitCards.getState().graph).toBe(false);
  useGitCards.getState().setOpen("graph", true);
  expect(useGitCards.getState().graph).toBe(true);
});
