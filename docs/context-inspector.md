# Context inspector

Open **Workspace → Context**, or click context usage in the status bar. The tab follows the
active lane and works with both provider adapters. It inspects sessions without sending a
message, changing skills, or requesting compaction.

## Reading the numbers

Context occupancy describes the latest reported window, not the sum of tokens billed across
turns. Capacity comes from the provider; unknown capacity stays unknown. Claude's summary
report is an estimate against its compaction window. Codex reports usage and capacity but
does not supply the same category breakdown. Deferred tools are outside occupancy math.

On-disk file estimates use UTF-8 bytes divided by four, rounded up, and appear with `~`.
These estimates are never added to reported usage. Source previews are read-only, limited to
48 KiB, and restricted to discovered Markdown files in permitted project/configuration roots.

## Reading skill activity

- **Available:** discovered locally or reported in the provider inventory; loading is unconfirmed.
- **Description in context:** included in the provider's context report; this does not prove the body loaded.
- **Requested:** explicitly named in the user input; loading is unconfirmed.
- **Invoked / invocation failed:** a skill tool call was observed, including any reported failure.
- **Read observed:** a successful skill-file read was observed.
- **Declared preload:** a Claude member definition names the skill; runtime loading is unconfirmed.

Expand a skill for its trigger description, source, and recorded evidence. Selection rationale
is shown only through evidence, never invented. Events before compaction are historical and
do not prove current presence. Subagent observations are attributed separately from main-session
occupancy where the provider supplies identity.

## Session recovery and limitations

Managed runtimes publish context metadata through the project event stream, including between
turns. Latest observations are kept under the ignored `.keel/turns/context/` directory, with
bounded retention. Reopened sessions use these records and available transcript evidence.
Transcript recovery initially reads at most 2 MiB, then only appended bytes; older events may
be omitted. A terminal without a verified session ID displays an explicit unavailable state.

`GET /api/context` accepts lane, provider, session, checkout (`wt`), and an optional `refresh`.
Refresh inspects an existing runtime; it never starts one. `GET /api/context/source` previews
one registered source. Both use the daemon's existing authentication. Provider inspection
failures preserve saved observations and expose the limitation. No recurring context polling
or paid token-count requests are used.
