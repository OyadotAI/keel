// Response receipts can confirm only the exact question and answer that were submitted.
import { expect, it } from "vitest";
import { acknowledgeQuestion, emptyQuestion, pickQuestion, questionAnswer, writeQuestion } from "./question-drafts";
import type { Pending } from "./store";
const request: Pending = { id: "question", lane: "lane", tool: "AskUserQuestion", command: "", rules: [], session_id: "", input: { questions: [{ question: "Choose?", multiSelect: true }] } };

it("trying and removing options preserves the custom multiline response", () => {
  let draft = writeQuestion(emptyQuestion(request), 0, "My own\nresponse");
  draft = pickQuestion(pickQuestion(draft, 0, "A"), 0, "B");
  expect(questionAnswer(draft, 0)).toBe("A, B");
  draft = pickQuestion(draft, 0, "A");
  expect(questionAnswer(draft, 0)).toBe("B");
  expect(draft.values[0].custom).toBe("My own\nresponse");
  expect(questionAnswer(writeQuestion(draft, 0, draft.values[0].custom), 0)).toBe("My own\nresponse");
});

it("a stream receipt tolerates JSON key order but never erases a different answer", () => {
  const drafts = { question: { ...emptyQuestion(request), submitted: { answers: { b: "B", a: "A" } } } };
  expect(acknowledgeQuestion(drafts, "question", { answers: { a: "A", b: "B" } })).toEqual({});
  expect(acknowledgeQuestion(drafts, "question", { answers: { a: "Changed", b: "B" } })).toBe(drafts);
  expect(acknowledgeQuestion(drafts, "other", { answers: { a: "A", b: "B" } })).toBe(drafts);
});

it("late delivery of an older response cannot erase newly composed writing", () => {
  const original = { ...writeQuestion(emptyQuestion(request), 0, "Original"), submitted: { answers: { q: "Original" } } };
  const edited = writeQuestion(original, 0, "New writing");
  const drafts = { question: edited };
  expect(acknowledgeQuestion(drafts, "question", { answers: { q: "Original" } })).toBe(drafts);
  expect(drafts.question.values[0].custom).toBe("New writing");
});
