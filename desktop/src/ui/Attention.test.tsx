// Rendering checks pin honest coverage, escaped metadata and one contextual primary action.
import { expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { attentionItems, type AttentionSource } from "../attention";
import type { Lane } from "../store";
const fake = vi.hoisted(() => ({ state: {} as AttentionSource }));
vi.mock("../store", () => ({ useStore: (select: (state: AttentionSource) => unknown) => select(fake.state) }));
import { AttentionButton, AttentionDialog, AttentionRow } from "./Attention";

const lane = (id: string, over: Partial<Lane> = {}): Lane => ({ id, project: "/project", title: id, agent: "claude", known: true, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [], ...over });
function state(lanes: Lane[]) {
  fake.state = { order: ["/project"], projects: { "/project": { name: "Project", lanes: lanes.map((lane) => lane.id) } }, lanes: Object.fromEntries(lanes.map((lane) => [lane.id, lane])) };
}
const render = () => renderToStaticMarkup(<AttentionDialog close={() => {}} activate={() => {}} openProject={async () => {}} />);

it("exposes all live reasons, disconnection and a unique action name without embedding HTML", () => {
  state([lane("q", { title: '<img src=x onerror="alert(1)">', error: '<script>unsafe</script>', pending: [{ id: "p", lane: "q", session_id: "s", tool: "AskUserQuestion", command: "", input: {}, rules: [] }], disconnected: true })]);
  const html = render();
  expect(html).toContain("Needs your answer"); expect(html).toContain("Lane error");
  expect(html).toContain("Disconnected — details may be out of date.");
  expect(html).toContain("&lt;script&gt;unsafe&lt;/script&gt;"); expect(html).not.toContain("<script>"); expect(html).not.toContain("<img ");
  expect(html).toContain("Answer question — Project");
  expect(html).toContain('aria-modal="true"');
});

it("counts lanes and gives only the first row a primary action", () => {
  state([lane("q", { unseen: true, error: "error", pending: [{ id: "p", lane: "q", session_id: "s", tool: "Bash", command: "", input: {}, rules: [] }] }), lane("other", { unseen: true })]);
  expect(render()).toMatch(/Review request/);
  expect((render().match(/class="primary"/g) ?? []).length).toBe(1);
  expect(renderToStaticMarkup(<AttentionButton open={() => {}} />)).toContain('aria-label="Attention · 2 lanes"');
  const [item] = attentionItems(fake.state);
  expect(renderToStaticMarkup(<AttentionRow item={item} first={false} activate={() => {}} />)).not.toContain('class="primary"');
});

it("does not call an empty partial list all clear and offers a real first-use action", () => {
  state([lane("unloaded", { loaded: false })]);
  expect(render()).toContain("No loaded updates yet."); expect(render()).toContain("Some conversations haven’t loaded yet.");
  expect(render()).not.toContain("All clear");
  state([]); expect(render()).toContain("No attention items in loaded conversations.");
  fake.state.order = [];
  expect(render()).toContain("Open a project to see questions"); expect(render()).toContain("Open project…");
  expect((render().match(/class="primary"/g) ?? []).length).toBe(1);
});
