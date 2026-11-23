// A terminal becoming ready must respect a deliberate focus owner elsewhere in the workspace.
const CLAIMED_FOCUS = ".sidebar, .palette, [role=dialog], .strip, .side-panel, [data-attention-error], input, textarea:not(.xterm-helper-textarea)";

/** Connection and visibility events share one policy; explicit terminal focus leases stay separate. */
export function canAutofocusTerminal(active: Pick<Element, "closest"> | null): boolean {
  return !active?.closest(CLAIMED_FOCUS);
}

type FocusOrigin = Pick<Element, "closest" | "matches" | "addEventListener" | "removeEventListener">;
const SUPERSEDING_INPUT = ["blur", "pointerdown", "keydown"] as const;

/** A fresh shell may inherit its composer's focus until a newer interaction supersedes the request. */
export function terminalConnectionFocus(origin: FocusOrigin | null, shell: boolean) {
  const explicit = !!(shell && origin?.matches("[data-chat-input]"));
  let requested = explicit;
  const release = () => SUPERSEDING_INPUT.forEach((event) => origin?.removeEventListener(event, cancel));
  const cancel = () => { requested = false; release(); };
  if (requested) SUPERSEDING_INPUT.forEach((event) => origin?.addEventListener(event, cancel));
  return {
    allowed: (active: Pick<Element, "closest"> | null) => explicit ? requested && active === origin : canAutofocusTerminal(active),
    dispose: cancel,
  };
}
