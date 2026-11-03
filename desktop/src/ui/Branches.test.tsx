// Busy checkout presentation explains disabled actions without blocking branch discovery.
import { beforeEach, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { Branches } from "../branches";
interface Lane { id: string; project: string; title: string; wt?: string; running?: boolean }
const fake = vi.hoisted(() => ({ lanes: {} as Record<string, Lane> }));
vi.mock("../store", () => ({ useStore: Object.assign((select: (state: typeof fake) => unknown) => select(fake), { getState: () => fake }) }));
import { BranchList } from "./Branches";
const data: Branches = { current: "main", local: [{ name: "main", current: true, ahead: 0, behind: 0 }, { name: "feature", current: false, ahead: 0, behind: 0 }], remotes: [], staged: 0, unstaged: 0 };
beforeEach(() => { fake.lanes = { current: { id: "current", project: "/scratch", title: "Current" } }; });
const render = (running = false, wt?: string, branches = data) => renderToStaticMarkup(<BranchList lane="current" ep={{ port: 1, token: "synthetic" }} branches={branches} pending={false} running={running} wt={wt} act={async () => true} />);
const expectDiscovery = (html: string) => {
  expect(html).toMatch(/<input[^>]*aria-label="Search branches"(?![^>]*disabled)/);
  expect(html).toContain('aria-label="Sort branches"');
  expect(html).not.toMatch(/<select[^>]*disabled/);
  expect(html).not.toMatch(/<button[^>]*aria-pressed[^>]*disabled/);
};

it("current checkout activity gets a prominent status above the count with mutations disabled", () => {
  const html = render(true);
  expect(html).toContain('role="status" aria-live="polite" aria-atomic="true"');
  expect(html).toContain("Branch switching is paused"); expect(html).toContain("Wait for its turn to finish");
  expect(html.indexOf("branch-busy-notice")).toBeLessThan(html.indexOf("branch-count"));
  expect(html).toMatch(/aria-label="Switch to feature" disabled=""/);
  expect(html).toMatch(/aria-label="Actions for feature" disabled=""/);
  expect(html).toContain('<button disabled="">Create</button>'); expectDiscovery(html);
});
it("only another lane on the same project and checkout triggers the warning", () => {
  fake.lanes.other = { id: "other", project: "/scratch", title: "Other", running: true };
  expect(render()).toContain("Branch switching is paused");
  fake.lanes.other.wt = "isolated"; expect(render()).not.toContain("branch-busy-notice");
  fake.lanes.other.wt = undefined; fake.lanes.other.project = "/unrelated";
  expect(render()).not.toContain("branch-busy-notice");
});
it("ownership keeps priority and server restrictions remain after activity clears", () => {
  const owned = render(true, "owned");
  expect(owned).toContain("This lane owns its branch"); expect(owned).not.toContain("branch-busy-notice");
  const restricted = { ...data, restriction: "Checkout temporarily unavailable." };
  expect(render(true, undefined, restricted)).not.toContain(restricted.restriction);
  const cleared = render(false, undefined, restricted);
  expect(cleared).not.toContain("branch-busy-notice"); expect(cleared).toContain(restricted.restriction);
  expect(cleared).toMatch(/aria-label="Switch to feature" disabled=""/);
  expect(render()).not.toMatch(/aria-label="Switch to feature" disabled/);
});
it("an empty busy list still explains the block and preserves discovery controls", () => {
  const html = render(true, undefined, { ...data, local: [] });
  expect(html).toContain("branch-busy-notice"); expect(html).toContain("No branches yet. Create the first commit"); expectDiscovery(html);
});
