import { expect, test } from "vitest";
import { groupHooks } from "./hooks";

test("a wrapper registered on many events is one row naming the script it runs", () => {
  const command = `if [ -z "\${HOME-}" ]; then :; else case "\${OSTYPE-}" in msys*) "\${HOME-}/.orca/agent-hooks/claude-hook.cmd" ;; *) /bin/sh "\${HOME-}/.orca/agent-hooks/claude-hook.sh"; esac; fi`;
  const rows = groupHooks(["PostCompact", "PermissionRequest", "PostCompact"].map((event) => ({ event, command, scope: "user", source: "/Users/mk/.claude/settings.json" })));
  expect(rows).toHaveLength(1);
  expect(rows[0].events).toEqual(["PostCompact", "PermissionRequest"]);
  expect(rows[0].runs).toBe("Runs ~/.orca/agent-hooks/claude-hook.sh");
  expect(rows[0].owner).toBe("Orca");
});

test("a short command is shown as itself", () => {
  expect(groupHooks([{ event: "Stop", command: "say done", scope: "user", source: "x" }])[0].runs).toBe("say done");
});
