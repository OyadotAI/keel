# Chat interface

First-run setup, new-project setup and Settings include **Chat interface**. Formatted chat is the default for new installs. Choose Terminal to keep using each agent's interactive CLI. The preference is stored locally as `keel.chat-interface` (`chat` or `terminal`); saved lanes retain their own choice, and lanes from earlier versions retain Terminal.

An idle lane can switch from the Chat/Terminal toggle above its conversation, its right-click menu in the sidebar, or its header menu. You can also use **Apply to current lane** in Settings. Finish or stop a turn and resolve any unconfirmed submission first. Claude history is refreshed from its transcript on handoff. Codex sessions created in formatted chat retain their real thread ID and can resume in the terminal. Exit the Codex terminal before switching back to formatted chat. Older Codex terminal lanes lack a tracked thread ID, so keep those lanes open and create a new formatted-chat lane instead of discarding their history.

Formatted chat supports Markdown and code copying, expandable tool output and reasoning, file references, attachments, model selection, Plan/Build modes, provider approvals and questions, queued messages, Stop, and per-lane drafts. Enter sends; Shift+Enter inserts a newline. The virtualized transcript follows output only while at the bottom. Provider-specific interactive CLI screens remain available through Terminal; formatted chat does not claim full CLI command parity.

The daemon owns persistent provider processes: the Claude Agent SDK and Codex App Server. UI disconnects do not stop turns. Submissions use durable IDs, and reconnecting clients replay ordered events from a private journal outside the repository. A daemon restart marks an unfinished turn interrupted instead of rerunning it. Repeating an unconfirmed submission uses its original ID.

## Development

`make dev` stages the helper and a local Node executable automatically. Release workflows stage checksum-verified standalone Node binaries; users do not need Node or pnpm. They still need the selected provider CLI and its authentication.

- `make check`: Rust, frontend, provider-adapter tests and bundle budget.
- `cd desktop && pnpm test:runtime`: isolated Codex protocol tests.
- After `cargo build -p keel` and `pnpm runtime:build` in `desktop/`, `python3 desktop/scripts/chat/smoke.py` exercises both authenticated providers through the daemon. This optional check consumes model tokens; pass `claude` or `codex` to test one.
- With Vite running, `/scripts/chat/check.html` provides a deterministic 300-turn, three-lane UI fixture. It mocks sending and never launches an agent.

Runtime source lives in `desktop/runtime/`; its generated bundle and staged binaries are ignored. `pnpm runtime:stage <target-triple>` prepares a release target; no argument uses the developer's local Node binary.
