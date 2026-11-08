import { expect, it, vi } from "vitest";
vi.mock("./store", () => ({ AGENT_NAME: { claude: "Claude", codex: "Codex", antigravity: "Antigravity" } }));
import { matchSessions } from "./history-search";
import type { Session } from "./store";

const s = (id: string, over: Partial<Session> = {}): Session => ({ id, messages: 1, live: false, busy: false, ...over });
const list = [s("a", { title: "Fix the login bug", branch: "keel/login" }), s("b", { title: "Subagent grid", provider: "codex" }), s("c")];

it("matches title, branch and agent, every word, ignoring case", () => {
  expect(matchSessions(list, "").map((x) => x.id)).toEqual(["a", "b", "c"]);
  expect(matchSessions(list, "LOGIN").map((x) => x.id)).toEqual(["a"]);
  expect(matchSessions(list, "keel/login").map((x) => x.id)).toEqual(["a"]);
  expect(matchSessions(list, "codex grid").map((x) => x.id)).toEqual(["b"]);
  expect(matchSessions(list, "untitled").map((x) => x.id)).toEqual(["c"]);
  expect(matchSessions(list, "grid login")).toEqual([]);
});
