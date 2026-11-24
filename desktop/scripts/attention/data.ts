// Synthetic responses preserve the real API contracts without contacting a daemon.
import type { Branches } from "../../src/branches";

const BRANCHES: Branches = {
  current: "keel/attention-qa",
  local: [{ name: "keel/attention-qa", current: true, ahead: 0, behind: 0 }],
  remote: [], remote_details: [], remotes: [], staged: 0, unstaged: 0,
};

/** Read-only fixtures keep unrelated App surfaces mounted during Attention verification. */
export function attentionFixtureData(path: string): unknown {
  const data: Record<string, unknown> = {
    "/api/git/status": { is_repo: true, branch: "keel/attention-qa", changes: [], head: "fixture-head" },
    "/api/git/branches": BRANCHES,
    "/api/dev": { running: false, log: [] },
    "/api/prereview": { confirmed: true, form: { title: "", draft: false }, target: { branch: "keel/attention-qa", base: "main" } },
    "/api/file": { text: "Synthetic fixture" },
  };
  return data[path] ?? [];
}
