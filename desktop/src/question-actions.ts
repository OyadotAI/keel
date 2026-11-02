// One delivery path preserves question drafts until the live provider or MCP waiter accepts them.
import { post, type Endpoint } from "./api";
import { requestChatFocus } from "./chat-focus";
import { emptyQuestion, pickQuestion, writeQuestion, questions, questionAnswer, recoveryMessage, type QuestionDraft } from "./question-drafts";
import type { Lane, Pending } from "./store";

interface Dependencies {
  read(): { lanes: Record<string, Lane>; projects: Record<string, { endpoint?: Endpoint }> };
  patch(id: string, patch: Partial<Lane> | ((lane: Lane) => Partial<Lane>)): void;
  persist(): boolean;
  draft(id: string, text: string): void;
}

/** Keep editing, retry, and recovery scoped to the original lane and request. */
export function createQuestionActions(deps: Dependencies) {
  function patch(id: string, request: Pending, update: (draft: QuestionDraft) => QuestionDraft) {
    deps.patch(id, (l) => ({ questionDrafts: { ...l.questionDrafts, [request.id]: update(l.questionDrafts?.[request.id] ?? emptyQuestion(request)) } }));
    deps.persist();
  }
  function dismissQuestion(id: string, request: string) {
    deps.patch(id, (l) => ({ questionDrafts: Object.fromEntries(Object.entries(l.questionDrafts ?? {}).filter(([key]) => key !== request)) }));
    deps.persist();
  }
  async function answerQuestion(id: string, request: Pending): Promise<boolean> {
    const lane = deps.read().lanes[id];
    const draft = lane?.questionDrafts?.[request.id];
    const endpoint = lane && deps.read().projects[lane.project]?.endpoint;
    if (!draft || draft.sending) return false;
    if (!endpoint) { patch(id, request, (d) => ({ ...d, error: "Reconnect to deliver your answer." })); return false; }
    if (!questions(request).length || questions(request).some((_, i) => !questionAnswer(draft, i).trim())) return false;
    const body = submission(draft, id, lane.session);
    patch(id, request, (d) => ({ ...d, sending: true, error: undefined, submitted: body.answer }));
    try {
      const result = await post<{ ok: boolean; closed?: boolean; reason?: string }>(endpoint, body.route, body.data);
      // A stream receipt may already have acknowledged this exact answer while HTTP was pending.
      if (!deps.read().lanes[id]?.questionDrafts?.[request.id]) return true;
      if (!result.ok) {
        patch(id, request, (d) => ({ ...d, closed: true, sending: false, error: result.reason ?? "This question is no longer waiting." }));
        deps.patch(id, (l) => ({ pending: l.pending.filter((p) => p.id !== request.id) }));
        return false;
      }
      dismissQuestion(id, request.id);
      deps.patch(id, (l) => ({ pending: l.pending.filter((p) => p.id !== request.id) }));
      return true;
    } catch {
      if (!deps.read().lanes[id]?.questionDrafts?.[request.id]) return true;
      patch(id, request, (d) => ({ ...d, sending: false, error: "Couldn’t deliver your answer." }));
      return false;
    }
  }
  function useQuestionAsMessage(id: string, request: string) {
    const lane = deps.read().lanes[id];
    const draft = lane?.questionDrafts?.[request];
    if (!draft) return;
    deps.draft(id, [lane.draft, recoveryMessage(draft)].filter((text) => text?.trim()).join("\n\n"));
    dismissQuestion(id, request); requestChatFocus(id);
  }
  return {
    writeQuestion(id: string, request: Pending, index: number, custom: string) { patch(id, request, (d) => writeQuestion(d, index, custom)); },
    pickQuestion(id: string, request: Pending, index: number, option: string) { patch(id, request, (d) => pickQuestion(d, index, option)); },
    answerQuestion, dismissQuestion, useQuestionAsMessage,
  };
}

function submission(draft: QuestionDraft, lane: string, session?: string) {
  const request = draft.request;
  const answers = Object.fromEntries(questions(request).map((q, i) => [request.provider === "codex" ? q.id ?? q.question : q.question, questionAnswer(draft, i)]));
  if (request.provider) {
    const answer = { answers };
    return { route: "/api/chat/control", answer, data: { lane, method: "answer", id: request.id, answer } };
  }
  const answer = questions(request).map((q, i) => `${q.question}: ${questionAnswer(draft, i)}`).join("\n");
  return { route: "/api/approve/answer", answer, data: { id: request.id, decision: "deny", session: request.session_id || session, rules: [], scope: "session", answer } };
}
