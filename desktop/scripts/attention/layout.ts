// Browser-fixture assertions measure real request hit targets instead of inspecting CSS text.

/** Every enabled decision control must become reachable through ordinary scrolling. */
export function verifyRequestControls(page: Document): string {
  const dock = page.querySelector<HTMLElement>(".chat-dock:has([data-attention-request])");
  const controls = [...(dock?.querySelectorAll<HTMLElement>("[data-attention-request] button:not(:disabled), [data-attention-request] textarea:not(:disabled), [data-attention-request] input:not(:disabled), [data-attention-request] select:not(:disabled)") ?? [])];
  if (!dock || !controls.length) return "FAIL: no enabled request controls to check";
  const before = dock.scrollTop, missed: string[] = [];
  for (const control of controls) {
    control.scrollIntoView({ block: "center", inline: "nearest", behavior: "instant" });
    const rect = control.getBoundingClientRect();
    const hit = page.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
    if (!rect.width || !rect.height || !hit || !control.contains(hit)) missed.push(control.getAttribute("aria-label") || control.textContent?.trim() || control.tagName);
  }
  dock.scrollTop = before;
  return missed.length ? `FAIL: ${missed.length}/${controls.length} request controls unreachable: ${missed.join(", ")}` : `PASS: ${controls.length} request controls reachable by scrolling`;
}
