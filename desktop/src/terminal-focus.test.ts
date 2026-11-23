// Late terminal readiness must not override a review, request, modal, or input focus owner.
import { expect, it, vi } from "vitest";
import { canAutofocusTerminal, terminalConnectionFocus } from "./terminal-focus";

it.each([".side-panel", ".strip", "[role=dialog]", "[data-attention-error]", "input", ".sidebar"])("preserves focus owned by %s when a terminal becomes ready", (owner) => {
  const closest = vi.fn((selector: string) => selector.split(", ").includes(owner) ? {} as Element : null);
  expect(canAutofocusTerminal({ closest })).toBe(false);
});

it("allows an unclaimed workspace and an existing terminal input to receive automatic focus", () => {
  expect(canAutofocusTerminal(null)).toBe(true);
  expect(canAutofocusTerminal({ closest: () => null })).toBe(true);
});

function composer() {
  return Object.assign(new EventTarget(), { matches: () => true, closest: () => ({} as Element) });
}

it("a fresh shell takes its original composer's focus while an unsolicited agent connection does not", () => {
  const origin = composer();
  const shell = terminalConnectionFocus(origin, true), agent = terminalConnectionFocus(origin, false);
  expect(shell.allowed(origin)).toBe(true); expect(agent.allowed(origin)).toBe(false);
  shell.dispose(); agent.dispose();
});

it.each(["blur", "pointerdown", "keydown"])("a newer %s cancels the shell's focus request even if the composer is focused again", (event) => {
  const origin = composer(), request = terminalConnectionFocus(origin, true);
  origin.dispatchEvent(new Event(event));
  expect(request.allowed(origin)).toBe(false);
  expect(request.allowed(null)).toBe(false);
  request.dispose();
});

it("Attention evidence and cleanup supersede a pending shell's focus request", () => {
  const origin = composer(), request = terminalConnectionFocus(origin, true);
  expect(request.allowed({ closest: () => ({} as Element) })).toBe(false);
  request.dispose();
  expect(request.allowed(origin)).toBe(false);
});
