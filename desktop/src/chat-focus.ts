// New conversations and recovered drafts request focus only after their input has mounted.

/** A pending user action survives mounting without stealing focus on routine lane switches. */
const requested = new Set<string>();

/** Remember the explicit action until the new conversation's input can receive focus. */
export function requestChatFocus(lane: string): void {
  requested.add(lane);
  if (typeof document === "undefined") return;
  requestAnimationFrame(() => {
    const input = document.querySelector<HTMLElement>(`[data-chat-input="${CSS.escape(lane)}"]`);
    if (input) consumeChatFocus(lane, input);
  });
}

/** Focus native chat once; terminal focus is retried when its connected mount becomes ready. */
export function consumeChatFocus(lane: string, input: { focus(): void }): void {
  if (!requested.has(lane)) return;
  input.focus(); requested.delete(lane);
}
