import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(async () => true), requestPermission: vi.fn(), sendNotification: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ setBadgeCount: vi.fn(async () => undefined), requestUserAttention: vi.fn(async () => undefined) }), UserAttentionType: { Informational: 2 } }));

import { endBanner, turnEnded } from "./notify";

describe("what a finished turn's banner says", () => {
  it("says how long it took and what the checks said", () => {
    expect(turnEnded(192_000, { status: "passed" })).toBe("Done in 3m 12s. Checks passed.");
    expect(turnEnded(192_000, { status: "failed", problems: [{}, {}, {}, {}] })).toBe("Done in 3m 12s. Checks failed: 4 problems.");
    expect(turnEnded(9_000, { status: "failed", problems: [{}] })).toBe("Done in 9s. Checks failed: 1 problem.");
    expect(turnEnded(45_000, undefined)).toBe("Done in 45s.");
    expect(turnEnded(undefined, { status: "none" })).toBe("Done.");
  });

  it("says nothing at the end of a turn that was stopped, interrupted or failed", () => {
    for (const closed of ["stopped", "interrupted", "failed"]) expect(endBanner({ closed, ms: 41_000 })).toBeUndefined();
    expect(endBanner({ closed: "done", ms: 41_000, gate: { status: "passed" } })).toBe("Done in 41s. Checks passed.");
    expect(endBanner({ ms: 41_000 })).toBe("Done in 41s.");
  });
});
