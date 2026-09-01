import XCTest
@testable import KeelApp

/// The daemon forwards Claude Code's `stream-json` lines verbatim — it translates nothing (see the
/// comment in api.rs). So this parser is the only thing standing between the wire and the stage,
/// and every record shape it can meet is worth pinning down.
@MainActor
final class StreamTests: XCTestCase {

    private func model() -> SessionModel {
        SessionModel(client: Client(port: 0))
    }

    private func feed(_ json: String, _ m: SessionModel, _ t: Turn) {
        m.record(Data(json.utf8), into: t)
    }

    func testInitAnnouncesTheSession() {
        let m = model(), t = Turn(prompt: "hi")
        XCTAssertNil(m.sessionId)
        feed(#"{"type":"system","subtype":"init","session_id":"abc12345-de","model":"opus"}"#, m, t)
        XCTAssertEqual(m.sessionId, "abc12345-de")
    }

    /// A lane is named by what it was asked to do.
    ///
    /// Naming it from the session id gave every lane a hex string, and naming none of them gave
    /// three concurrent agents three rows reading "New session" — which is what made running
    /// several of them look pointless. The first ask is the thing that tells them apart.
    func testALaneIsNamedByItsFirstAsk() {
        let m = model()
        XCTAssertEqual(m.title, "Untitled")

        m.prompt = "Fix the failing billing tests\nand explain what broke"
        m.send()
        XCTAssertEqual(m.title, "Fix the failing billing tests",
                       "the first line names it, not the whole prompt")

        // A later ask must not rename it: the name is how you found this lane again.
        let named = m.title
        m.prompt = "now do something else"
        m.send()
        XCTAssertEqual(m.title, named)
    }

    /// A very long first line is a title, not a paragraph.
    func testALongAskIsTruncatedIntoATitle() {
        let m = model()
        m.prompt = String(repeating: "a", count: 200)
        m.send()
        XCTAssertEqual(m.title.count, 60)
    }

    func testTextAndThinkingAccumulate() {
        let m = model(), t = Turn(prompt: "hi")
        feed(#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"Hel"}}}"#, m, t)
        feed(#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"lo"}}}"#, m, t)
        feed(#"{"type":"stream_event","event":{"delta":{"type":"thinking_delta","thinking":"hm"}}}"#, m, t)
        XCTAssertEqual(t.text, "Hello")
        XCTAssertEqual(t.thinking, "hm")
    }

    /// A write tool is how the stage learns a file changed, and the subject is picked from the
    /// input key that tool actually uses.
    func testAToolCallBecomesARowAndAChangedFile() {
        let m = model(), t = Turn(prompt: "edit")
        feed(#"""
        {"type":"assistant","message":{"content":[
          {"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"src/api.rs"}},
          {"type":"tool_use","id":"t2","name":"Bash","input":{"command":"cargo test\nsecond line"}}
        ]}}
        """#, m, t)
        XCTAssertEqual(t.files, ["src/api.rs"], "only write tools count as a changed file")
        XCTAssertEqual(t.calls.map(\.tool), ["Edit", "Bash"])
        XCTAssertEqual(t.calls[0].subject, "src/api.rs")
        XCTAssertEqual(t.calls[1].subject, "cargo test", "a multi-line command shows its first line")
    }

    /// A `tool_result` is a bare string on some records and a list of text blocks on others. Both
    /// have to land, or half the outputs in a turn render blank.
    func testAResultPairsWithItsCallInEitherShape() {
        let m = model(), t = Turn(prompt: "run")
        feed(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#, m, t)
        feed(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"a.rs"}}]}}"#, m, t)

        feed(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a.rs b.rs"}]}}"#, m, t)
        feed(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"fn main"}],"is_error":true}]}}"#, m, t)

        XCTAssertEqual(t.calls[0].output, "a.rs b.rs")
        XCTAssertFalse(t.calls[0].running)
        XCTAssertEqual(t.calls[1].output, "fn main")
        XCTAssertTrue(t.calls[1].failed)
    }

    /// Both panes follow their tail by watching one token change. Everything that makes a running
    /// turn taller has to be in it, or the pane sits still while the turn grows under it and you
    /// scroll to the end by hand — which is what a landing `tool_result` and a subagent's rows
    /// both did: neither changes a count, and both are most of what a turn produces.
    func testTheTailTokenMovesWithEverythingARunningTurnAdds() {
        let m = model(), t = Turn(prompt: "run")
        m.turns.append(t)

        var seen = [m.tailToken]
        func moved(_ what: String) {
            XCTAssertFalse(seen.contains(m.tailToken), "\(what) left the tail token unchanged")
            seen.append(m.tailToken)
        }

        feed(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Task","input":{"description":"look"}}]}}"#, m, t)
        moved("a call starting")
        feed(#"{"type":"assistant","parent_tool_use_id":"t1","message":{"content":[{"type":"tool_use","id":"c1","name":"Read","input":{"file_path":"a.rs"}}]}}"#, m, t)
        moved("a subagent's call")
        feed(#"{"type":"user","parent_tool_use_id":"t1","message":{"content":[{"type":"tool_result","tool_use_id":"c1","content":"fn main"}]}}"#, m, t)
        moved("a subagent's output")
        feed(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"ls"}}]}}"#, m, t)
        moved("a second call")
        // Out of order on purpose: the newest call is `t2`, and it is `t1` that grew the card.
        feed(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"forty lines of it"}]}}"#, m, t)
        moved("output landing on a call that is not the newest")
        feed(#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}}}"#, m, t)
        moved("a thinking delta")
        feed(#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Done."}}}"#, m, t)
        moved("a text delta")
    }

    func testResultCarriesCostAndDuration() {
        let m = model(), t = Turn(prompt: "x")
        feed(#"{"type":"result","total_cost_usd":0.31,"duration_ms":134000,"session_id":"s1"}"#, m, t)
        XCTAssertEqual(t.cost, 0.31)
        XCTAssertEqual(t.durationMS, 134000)
        XCTAssertEqual(m.sessionId, "s1")
    }

    /// A turn that reads twenty files is one row, not twenty.
    ///
    /// The first version of this only collapsed a call whose predecessor had produced no output
    /// yet — true while a turn streams, never true for a replayed session. A 746-message session
    /// therefore rendered three hundred near-identical `Bash` rows and the window was unreadable.
    /// Grouping is about the tool, not about whether the answer has arrived.
    func testConsecutiveIdenticalToolsCollapseEvenWithOutput() {
        let m = model(), t = Turn(prompt: "x")
        for i in 1...3 {
            feed(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"r\#(i)","name":"Read","input":{"file_path":"f\#(i).rs"}}]}}"#, m, t)
            // Every call answered, which is the state a replay is always in.
            feed(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"r\#(i)","content":"contents"}]}}"#, m, t)
        }
        feed(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"make"}}]}}"#, m, t)

        let groups = t.groups
        XCTAssertEqual(groups.count, 2, "answered calls still group")
        XCTAssertEqual(groups[0].tool, "Read")
        XCTAssertEqual(groups[0].calls.count, 3)
        XCTAssertEqual(groups[1].tool, "Bash")
    }

    /// A failure inside a run must not split it into three rows, but the group has to say so.
    func testAFailureIsCarriedByItsGroupRatherThanSplittingIt() {
        let m = model(), t = Turn(prompt: "x")
        for i in 1...3 {
            feed(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"c\#(i)","name":"Bash","input":{"command":"step \#(i)"}}]}}"#, m, t)
        }
        feed(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"c2","content":"boom","is_error":true}]}}"#, m, t)

        XCTAssertEqual(t.groups.count, 1)
        XCTAssertTrue(t.groups[0].failed, "the group reports the failure inside it")
        XCTAssertEqual(t.groups[0].calls.count, 3)
    }

    /// Different tools stay apart, or the row stops meaning anything.
    func testDifferentToolsDoNotMerge() {
        let m = model(), t = Turn(prompt: "x")
        for (i, tool) in ["Read", "Edit", "Read"].enumerated() {
            feed(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"x\#(i)","name":"\#(tool)","input":{"file_path":"a.rs"}}]}}"#, m, t)
        }
        XCTAssertEqual(t.groups.map(\.tool), ["Read", "Edit", "Read"])
    }

    /// Garbage on the wire must not take the window down with it.
    func testAnUnreadableRecordIsIgnored() {
        let m = model(), t = Turn(prompt: "x")
        feed("{ not json", m, t)
        feed(#"{"type":"something_new_we_do_not_know"}"#, m, t)
        XCTAssertTrue(t.calls.isEmpty)
        XCTAssertEqual(t.text, "")
    }

    func testCodexEventsBecomeReviewEvidence() {
        let m = model(), t = Turn(prompt: "fix it")
        m.provider = .codex

        feed(#"{"type":"thread.started","thread_id":"thread-1"}"#, m, t)
        feed(#"{"type":"item.started","item":{"id":"cmd-1","type":"command_execution","command":"swift test","status":"in_progress"}}"#, m, t)
        feed(#"{"type":"item.completed","item":{"id":"cmd-1","type":"command_execution","command":"swift test","aggregated_output":"All tests passed","exit_code":0,"status":"completed"}}"#, m, t)
        feed(#"{"type":"item.completed","item":{"id":"patch-1","type":"file_change","changes":[{"path":"Sources/App.swift","kind":"update"}],"status":"completed"}}"#, m, t)
        feed(#"{"type":"item.completed","item":{"id":"message-1","type":"agent_message","text":"Fixed the race."}}"#, m, t)
        feed(#"{"type":"turn.completed","usage":{"input_tokens":120,"cached_input_tokens":20,"output_tokens":30}}"#, m, t)

        XCTAssertEqual(m.sessionId, "thread-1")
        XCTAssertEqual(t.calls.count, 1)
        XCTAssertEqual(t.calls[0].subject, "swift test")
        XCTAssertEqual(t.calls[0].output, "All tests passed")
        XCTAssertFalse(t.calls[0].failed)
        XCTAssertEqual(t.files, ["Sources/App.swift"])
        XCTAssertEqual(t.text, "Fixed the race.")
        XCTAssertEqual(t.tokens, Turn.Tokens(input: 120, output: 30, cacheRead: 20, cacheWrite: 0))
    }

    func testCodexFailuresSurfaceWithoutCrashingTheStream() {
        let m = model(), t = Turn(prompt: "fix it")
        m.provider = .codex

        feed(#"{"type":"turn.failed","error":{"message":"sandbox denied the command"}}"#, m, t)

        XCTAssertEqual(m.lastError, "sandbox denied the command")
    }

    /// Review notes are keyed by file and line, and survive the view that made them.
    func testCommentsBecomeAPrompt() {
        let m = model()
        m.notes["src/api.rs:530"] = "reuse the spawn helper"
        m.notes["src/serve.rs:12"] = "this route is dead"
        let p = m.commentsPrompt()
        XCTAssertTrue(p.hasPrefix("Review comments on the changes you just made:"))
        XCTAssertTrue(p.contains("src/api.rs:530\nreuse the spawn helper"))
        XCTAssertTrue(p.contains("src/serve.rs:12\nthis route is dead"))
    }
}

/// The wire format under the stream-json. Keel's own SSE is two lines and a blank one, and getting
/// the blank-line reset wrong is how every event after the first ends up mislabelled.
final class SSETests: XCTestCase {
    private func parse(_ text: String) -> [Client.Event] {
        var p = SSEParser()
        return text.split(separator: "\n", omittingEmptySubsequences: false)
            .compactMap { p.feed(String($0)) }
    }

    func testNamedEventsAndTheBlankLineReset() {
        let events = parse("""
        event: msg
        data: {"type":"result"}

        event: done
        data: 0

        data: unnamed
        """)
        XCTAssertEqual(events.map(\.name), ["msg", "done", "message"])
        XCTAssertEqual(events[0].data, #"{"type":"result"}"#)
        XCTAssertEqual(events[1].data, "0")
        XCTAssertEqual(events[2].data, "unnamed", "a data line after a reset is not still `done`")
    }

    func testOnlyTheFirstSpaceAfterTheColonIsPadding() {
        XCTAssertEqual(parse("data:  two spaces").first?.data, " two spaces")
        XCTAssertEqual(parse("data:{\"tight\":1}").first?.data, #"{"tight":1}"#)
    }

    /// An `event:` with no `data:` is still an event.
    ///
    /// This one cost an evening. axum writes **no `data:` line at all** when the payload is empty,
    /// so `/api/session/tail`'s `caught-up` — the signal that says a replayed conversation is
    /// complete and can be drawn — went out as a name followed by a blank line. The parser only
    /// ever yielded on `data:`, so the event vanished between two layers that each looked
    /// correct: the daemon's own stream showed `event: caught-up` on the wire, and the pane sat on
    /// "Opening this session…" forever with the whole transcript already decoded behind it.
    ///
    /// Checking the wire is not checking the parser. Both ends are asserted here.
    func testAnEventWithNoDataIsStillAnEvent() {
        XCTAssertEqual(parse("event: caught-up\n").map { [$0.name, $0.data] },
                       [["caught-up", ""]])
        // And it does not double-fire for an event that did carry data.
        XCTAssertEqual(parse("event: msg\ndata: {}\n").map(\.name), ["msg"])
        // A blank line on its own, with no event pending, is nothing.
        XCTAssertTrue(parse("\n\n").isEmpty)
    }

    /// A comment line is the server's heartbeat, and it is worth more than nothing.
    ///
    /// It used to be dropped with `id:` and `retry:`, which are genuinely noise. But "the daemon
    /// is still there" and "the agent is thinking" look identical to a reader that sees no bytes,
    /// and the chat stream's own timeout is an hour — so a daemon that died presented as
    /// "thinking…" until somebody gave up on it. Surfaced under a name no handler can send, so a
    /// consumer that does not care ignores it in one line.
    func testAKeepAliveIsSurfacedAndTheRestIsNoise() {
        XCTAssertEqual(parse(":keep-alive").map(\.name), [Client.keepAlive])
        XCTAssertTrue(parse("id: 7\nretry: 100").isEmpty)
        XCTAssertFalse(Client.keepAlive.allSatisfy(\.isLetter),
                       "it must not be spellable as an event name a handler could send")
    }
}

/// The wire types decode what the daemon actually sends, and survive what it might.
final class WireTests: XCTestCase {
    private func decode<T: Decodable>(_ json: String, _ type: T.Type) throws -> T {
        try JSONDecoder().decode(type, from: Data(json.utf8))
    }

    /// A hook that came with the checkout is marked, because it is a shell command someone else
    /// wrote that runs on the machine of whoever opens the repo.
    func testProjectScopeMeansItCameWithTheRepository() throws {
        let w = try decode(#"""
        {"sessions":[],"hooks":[
          {"event":"PreToolUse","command":"./danger.sh","scope":"project","source":".claude"},
          {"event":"Stop","command":"say done","scope":"user","source":""}
        ]}
        """#, Wire.Workspace.self)
        XCTAssertTrue(w.hooks[0].fromRepo)
        XCTAssertFalse(w.hooks[1].fromRepo)
    }

    /// The daemon can grow or drop a field. A client that refuses the whole response over one
    /// absent key shows nothing at all, which is worse than showing a shorter list.
    func testAbsentListsDecodeAsEmptyRatherThanFailing() throws {
        let w = try decode(#"{"sessions":[]}"#, Wire.Workspace.self)
        XCTAssertTrue(w.skills.isEmpty)
        XCTAssertTrue(w.hooks.isEmpty)
        XCTAssertTrue(w.mcpServers.isEmpty)
    }

    func testAnAbsentDescriptionIsNotAFailure() throws {
        let n = try decode(#"{"name":"thing","scope":"user"}"#, Wire.Named.self)
        XCTAssertEqual(n.description, "")
        XCTAssertEqual(n.name, "thing")
    }

    /// `mcp_servers` is snake_case on the wire and must not silently arrive empty.
    func testSnakeCaseKeysAreMapped() throws {
        let w = try decode(#"{"sessions":[],"mcp_servers":[{"name":"linear","scope":"project"}]}"#,
                           Wire.Workspace.self)
        XCTAssertEqual(w.mcpServers.count, 1)
        XCTAssertTrue(w.mcpServers[0].fromRepo)
    }

    /// A replayed turn's footer, from the records a transcript actually contains.
    ///
    /// The trap this is written against: a transcript has **no `result` record at all**. Checked
    /// against every `.jsonl` in this project — zero `result`, and 9,420 `assistant` records
    /// carrying `message.usage`. A first version of this test fed a hand-written `result` line and
    /// passed green against a shape the reader will never see, while every session opened from
    /// History drew no footer: no time, no tokens, no cache share. So the usage is accumulated per
    /// request and the elapsed time is the span of the turn's own timestamps.
    @MainActor
    func testAReplayedTurnGetsItsFooterFromWhatATranscriptContains() {
        let m = SessionModel(client: Client(port: 0))
        let t = Turn(prompt: "x")
        t.replayed = true
        func assistant(_ stamp: String, _ input: Int, _ output: Int, cacheRead: Int) -> Data {
            Data(#"""
            {"type":"assistant","timestamp":"\#(stamp)","message":{"content":[],
             "usage":{"input_tokens":\#(input),"output_tokens":\#(output),
                      "cache_read_input_tokens":\#(cacheRead),"cache_creation_input_tokens":0}}}
            """#.utf8)
        }
        m.record(assistant("2026-01-01T10:00:00.000Z", 10, 5, cacheRead: 400), into: t)
        m.record(assistant("2026-01-01T10:00:41.000Z", 4, 6, cacheRead: 500), into: t)

        XCTAssertEqual(t.tokens?.output, 11, "accumulated across the turn's requests")
        XCTAssertEqual(t.tokens?.cacheRead, 900)
        XCTAssertEqual(t.durationMS, 41_000, "the span of its own records")
        // The other shape, from a transcript written without fractional seconds.
        XCTAssertNotNil(SessionModel.moment("2026-01-01T10:00:00Z"))
    }

    /// A failure in a transcript already happened; it is not something to act on now.
    ///
    /// `classify` matches `message.model == "<synthetic>"`, which is exactly what the CLI writes
    /// into a transcript — so routing a replay through the live reader popped a red banner with a
    /// **Retry** button for a rate limit that reset weeks ago, and reported it to Sentry as a turn
    /// that had just failed. Seven of them in this project's own history.
    @MainActor
    func testAFailureInATranscriptIsHistoryRatherThanNews() {
        let m = SessionModel(client: Client(port: 0))
        let old = Turn(prompt: "then"); old.replayed = true
        let now = Turn(prompt: "now")
        let spent = Data(#"""
        {"type":"assistant","error":"rate_limit","message":{"model":"<synthetic>",
         "content":[{"type":"text","text":"You've hit your session limit"}]}}
        """#.utf8)

        m.record(spent, into: old)
        XCTAssertNotNil(old.failure, "still recorded on the turn it happened to")
        XCTAssertNil(m.lastError, "but not raised as something wrong now")

        m.record(spent, into: now)
        XCTAssertNotNil(now.failure)
        XCTAssertNotNil(m.lastError, "a live one still is")
    }

    /// Superseded by the two above; kept only so the live `result` path stays covered.
    @MainActor
    func testALiveResultStillCarriesTheAuthoritativeTotal() {
        let m = SessionModel(client: Client(port: 0))
        let t = Turn(prompt: "x")
        m.record(Data(#"""
        {"type":"result","duration_ms":41000,"total_cost_usd":0.12,
         "usage":{"input_tokens":10,"output_tokens":5,
                  "cache_read_input_tokens":900,"cache_creation_input_tokens":100}}
        """#.utf8), into: t)

        XCTAssertEqual(t.durationMS, 41_000)
        XCTAssertEqual(t.cost, 0.12)
        XCTAssertEqual([t.tokens?.cacheRead, t.tokens?.cacheWrite], [900, 100])
        // 900 of the 1,010 tokens read. The share is what makes the cost figure make sense:
        // a turn with 90% cache reads is cheap in a way its input count alone hides.
        XCTAssertEqual(t.tokens?.cached ?? 0, 900.0 / 1010.0, accuracy: 0.001)
    }

    /// The two shapes the monitoring feature travels in. A field renamed on the daemon side
    /// leaves the panel empty and the completion never reaches the conversation, and both
    /// failures are silent — the job still runs, nobody is told anything.
    func testABackgroundJobDecodesAndKnowsWhetherItIsStillRunning() throws {
        let running = try decode(#"{"id":"m1","lane":"L","command":"gh run watch 1","dir":"/repo","started":1000,"finished":null,"exit":null,"log":["queued"],"reported":false}"#, Wire.Job.self)
        XCTAssertTrue(running.running)

        let done = try decode(#"{"id":"m2","lane":"L","command":"true","dir":"/repo","started":1000,"finished":1090,"exit":0,"log":["ok"],"reported":false}"#, Wire.Job.self)
        XCTAssertFalse(done.running)
        XCTAssertEqual(done.exit, 0)
        XCTAssertEqual(done.elapsed, 90)
    }

    /// A monitor request is not a permission and must not be drawn as one: its answers decide who
    /// runs the command, not whether it is allowed.
    func testAMonitorRequestIsItsOwnKindOfQuestion() throws {
        let p = try decode(#"{"id":"bg-1","tool":"MonitorRequest","command":"gh run watch 1","rules":[],"session_id":"s"}"#, Wire.Pending.self)
        XCTAssertTrue(p.isMonitor)
        XCTAssertFalse(p.isQuestion)
        XCTAssertTrue(p.rules.isEmpty, "monitoring must never write a permission rule")
    }

    func testDiffLinesCarryBothLineNumbers() throws {
        let d = try decode(#"""
        {"path":"a.rs","untracked":false,"hunks":[{"header":"@@ -1 +1 @@","lines":[
          {"kind":"del","old":1,"new":null,"text":"was"},
          {"kind":"add","old":null,"new":1,"text":"is"}]}]}
        """#, Wire.Diff.self)
        XCTAssertEqual(d.hunks[0].lines[0].old, 1)
        XCTAssertNil(d.hunks[0].lines[0].new)
        XCTAssertEqual(d.hunks[0].lines[1].new, 1)
    }
}

/// What a running turn shows without being clicked.
@MainActor
final class LiveTurnTests: XCTestCase {

    private func running(_ toolCount: Int) -> Turn {
        let t = Turn(prompt: "x")
        for i in 0..<toolCount {
            t.begin(call: "c\(i)", tool: "Bash", input: ["command": .string("step \(i)")])
        }
        return t
    }

    /// A group with a call in flight is what is happening right now, so it must not need a click.
    func testARunningGroupIsOpenAndAFinishedOneIsNot() {
        let t = running(3)
        XCTAssertTrue(t.groups[0].running)

        for i in 0..<3 { t.finish(call: "c\(i)", output: "done", failed: false) }
        XCTAssertFalse(t.groups[0].running)
    }

    /// While it runs the tail matters; once finished you read from the top. Capping the wrong end
    /// means a live turn's newest work scrolls out of the window it just scrolled into.
    func testTheCapKeepsTheTailWhileRunning() {
        let t = running(20)
        let cap = 15
        let live = Array(t.groups.suffix(cap))
        t.finished = true
        let done = Array(t.groups.prefix(cap))

        // One group per distinct tool run — all Bash here, so they collapse to one.
        XCTAssertEqual(t.groups.count, 1)
        XCTAssertEqual(live.count, done.count)

        // With mixed tools the two ends genuinely differ, which is the case that matters.
        let mixed = Turn(prompt: "x")
        for i in 0..<20 {
            mixed.begin(call: "m\(i)", tool: i.isMultiple(of: 2) ? "Bash" : "Read",
                        input: ["command": .string("\(i)")])
        }
        XCTAssertEqual(mixed.groups.count, 20)
        XCTAssertNotEqual(Array(mixed.groups.prefix(cap)).map(\.id),
                          Array(mixed.groups.suffix(cap)).map(\.id))
    }
}

/// The turn in the order it happened, which is what the CLI shows and Keel did not.
@MainActor
final class StepsTests: XCTestCase {
    private func model() -> SessionModel { SessionModel(client: Client(port: 0)) }
    private func feed(_ json: String, _ m: SessionModel, _ t: Turn) {
        m.record(Data(json.utf8), into: t)
    }

    /// One `input_json_delta`, with the fragment escaped the way the wire escapes it.
    private func argue(_ fragment: String, index: Int, _ m: SessionModel, _ t: Turn) {
        let quoted = String(decoding: try! JSONEncoder().encode(fragment), as: UTF8.self)
        feed("""
        {"type":"stream_event","event":{"type":"content_block_delta","index":\(index),\
        "delta":{"type":"input_json_delta","partial_json":\(quoted)}}}
        """, m, t)
    }

    /// Prose, a command, and more prose is three things in a sequence.
    ///
    /// Keel merged every text delta of a turn into one string and put every call in a list beside
    /// it, so a turn that said "I'll check the router", grepped, and then explained what it found
    /// rendered as one paragraph and a separate command log. The sequence is most of what a person
    /// is reading for, and it could not be recovered from what was stored.
    func testProseAndCallsInterleave() {
        let m = model()
        let t = Turn(prompt: "check the router")

        feed(#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text"}}}"#, m, t)
        feed(#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"I'll check the router first."}}}"#, m, t)
        feed(#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#, m, t)

        feed(#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"c1","name":"Bash"}}}"#, m, t)
        // Split mid-string, the way the arguments actually arrive.
        argue(#"{"command": "rg -n"#, index: 1, m, t)
        argue(#" route"}"#, index: 1, m, t)
        feed(#"{"type":"stream_event","event":{"type":"content_block_stop","index":1}}"#, m, t)

        feed(#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text"}}}"#, m, t)
        feed(#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Found it."}}}"#, m, t)

        XCTAssertEqual(t.steps.count, 3)
        guard case .say = t.steps[0] else { return XCTFail("first is prose") }
        guard case .call(let id) = t.steps[1] else { return XCTFail("second is the call") }
        guard case .say = t.steps[2] else { return XCTFail("third is prose again") }
        XCTAssertEqual(id, "c1")
        XCTAssertEqual(t.text, "I'll check the router first.\n\nFound it.",
                       "the merged form the rest of the app reads is unchanged")
    }

    /// The whole command, not its first hundred and sixty characters.
    ///
    /// `begin` kept one field of the input, first line only, capped — so a heredoc showed the word
    /// `cat` and an `Edit` showed a path and never its hunks. The arguments arrive as
    /// `input_json_delta`, which was ignored outright.
    func testTheWholeToolInputSurvives() {
        let m = model()
        let t = Turn(prompt: "write it")
        let command = "cat <<'EOF' > a.txt\nline one\nline two\nEOF"

        feed(#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c1","name":"Bash"}}}"#, m, t)
        let payload = String(decoding: try! JSONEncoder().encode(["command": command]), as: UTF8.self)
        argue(payload, index: 0, m, t)
        feed(#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#, m, t)

        XCTAssertEqual(t.call("c1")?.input["command"]?.stringValue, command)
        XCTAssertEqual(t.call("c1")?.subject, "cat <<'EOF' > a.txt", "the row still shows one line")
        XCTAssertEqual(CallRow.parts(of: t.call("c1")!).first?.text, command,
                       "and the open row shows all of it")
    }

    /// The complete `assistant` message repeats every call the stream already announced. It is the
    /// repair, not a second call — before this it would have been drawn twice.
    func testTheCompleteMessageDoesNotDuplicateTheCall() {
        let m = model()
        let t = Turn(prompt: "read it")
        feed(#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c1","name":"Read"}}}"#, m, t)
        feed(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"c1","name":"Read","input":{"file_path":"/tmp/a.txt"}}]}}"#, m, t)

        XCTAssertEqual(t.calls.count, 1)
        XCTAssertEqual(t.steps.count, 1)
        XCTAssertEqual(t.call("c1")?.input["file_path"]?.stringValue, "/tmp/a.txt",
                       "and the complete message is what fills the arguments in")
    }

    /// A provider that does not stream partial messages still has to render.
    func testAWholeMessageStillBecomesASayStep() {
        let m = model()
        let t = Turn(prompt: "hello")
        feed(#"{"type":"assistant","message":{"content":[{"type":"text","text":"first"}]}}"#, m, t)
        feed(#"{"type":"assistant","message":{"content":[{"type":"text","text":"second"}]}}"#, m, t)
        XCTAssertEqual(t.steps.count, 2, "the second message is not swallowed by the first")
        XCTAssertEqual(t.text, "first\n\nsecond")
    }

    /// `rawCap` is 2,000 lines and partial messages emit one line per token, so the escape hatch
    /// that guarantees "there is no state in which Keel saw something and you cannot" used to fill
    /// with delta noise and drop every meaningful record behind it.
    func testDeltasDoNotFillTheRawLog() {
        let m = model()
        let t = Turn(prompt: "hi")
        for _ in 0..<50 {
            feed(#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}}"#, m, t)
        }
        feed(#"{"type":"result","is_error":false}"#, m, t)
        XCTAssertEqual(t.raw.count, 1, "only the records worth keeping")
    }
}

/// The Markdown parse, which used to run on every frame of a streaming reply.
@MainActor
final class MarkdownTests: XCTestCase {

    /// The parse belongs to the block, and runs on a clock rather than per token.
    ///
    /// Two versions of this were wrong in the same shape. First a cache keyed by the source
    /// string, which never hit — a streaming reply makes a *new* string per token, so every delta
    /// missed, paid a full-string hash, and evicted a finished turn's entry on the way out. Then
    /// re-parsing "just the current block" on every append, which is a smaller constant on the
    /// same O(n²) curve, because the current block *is* most of a reply. The parse is O(n) in the
    /// block; the fix is to run it twelve times a second, not once per token.
    func testABlockParsesOnSettleRatherThanPerToken() {
        let block = Turn.Block()
        for _ in 0..<100 { block.append("word ") }
        XCTAssertEqual(block.text.count, 500, "the text is current immediately")

        block.settle()
        XCTAssertEqual(block.blocks.count, 1)
        XCTAssertEqual(block.words, 100)
    }

    /// A settle with nothing new is free, so the tick costs nothing on an idle block.
    func testSettlingTwiceParsesOnce() {
        let block = Turn.Block("# Title")
        block.settle()
        XCTAssertEqual(block.blocks.count, 1)
        block.append("\n\nand a paragraph")
        block.settle()
        XCTAssertEqual(block.blocks.count, 2)
    }

    /// The text is never observed, so a token cannot invalidate a view on its own.
    ///
    /// The other half of the throttle: coalescing the *parse* buys nothing if the raw string
    /// still redraws the pane sixty times a second for a picture that cannot change.
    func testTheRawTextIsNotObserved() throws {
        let source = try String(contentsOfFile: #filePath
            .replacingOccurrences(of: "Tests/KeelAppTests/StreamTests.swift",
                                  with: "Sources/KeelApp/Turn.swift"), encoding: .utf8)
        XCTAssertTrue(source.contains("@ObservationIgnored private(set) var text"),
                      "Block.text must stay out of Observation")
    }

    /// Inline emphasis is resolved by the parser, not by `body`. `AttributedString(markdown:)`
    /// used to be called per paragraph per frame, which was the most expensive uncached thing on
    /// the render path.
    func testInlineIsResolvedAtParseTime() {
        guard case .paragraph(let text)? = Markdown.blocks("a **bold** word").first else {
            return XCTFail("no paragraph")
        }
        XCTAssertEqual(String(text.characters), "a bold word", "the markers are consumed")
    }

    func testTablesAndRulesAreBlocks() {
        let blocks = Markdown.blocks("""
        | a | b |
        |---|---|
        | 1 | 2 |

        ---

        after
        """)
        guard case .table(let rows) = blocks[0] else { return XCTFail("no table") }
        XCTAssertEqual(rows.count, 2)
        XCTAssertEqual(rows[0], ["a", "b"])
        XCTAssertEqual(rows[1], ["1", "2"])
        if case .rule = blocks[1] {} else { XCTFail("no rule") }
    }
}

// MARK: - Usage

@MainActor
final class UsageTests: XCTestCase {
    private func model() -> SessionModel { SessionModel(client: Client(port: 0)) }
    private func feed(_ json: String, _ m: SessionModel, _ t: Turn) {
        m.record(Data(json.utf8), into: t)
    }

    /// The CLI reports tokens; Keel shows them. The largest complaint cluster against the CLI is
    /// not knowing what a session is consuming, and every number needed is already in the stream.
    func testAResultCarriesTokensAndAnAssistantMessageCarriesContext() {
        let m = model(), t = Turn(prompt: "hi")
        feed(#"{"type":"assistant","message":{"usage":{"input_tokens":1200,"cache_read_input_tokens":140000,"cache_creation_input_tokens":800,"output_tokens":50},"content":[]}}"#, m, t)
        XCTAssertEqual(t.contextTokens, 142_000, "input plus everything read from cache")

        feed(#"{"type":"result","usage":{"input_tokens":3000,"output_tokens":900,"cache_read_input_tokens":27000,"cache_creation_input_tokens":0},"total_cost_usd":0.12,"duration_ms":60000}"#, m, t)
        XCTAssertEqual(t.tokens, Turn.Tokens(input: 3000, output: 900, cacheRead: 27000, cacheWrite: 0))
        XCTAssertEqual(t.tokens!.cached, 0.9, accuracy: 0.001)

        let t2 = Turn(prompt: "again")
        feed(#"{"type":"result","usage":{"input_tokens":1000,"output_tokens":100},"total_cost_usd":0.06,"duration_ms":30000}"#, m, t2)
        m.turns = [t, t2]
        XCTAssertEqual(m.sessionTokens?.total, 3000 + 900 + 27000 + 1000 + 100)
        XCTAssertEqual(m.contextTokens, 142_000, "the last turn that reported one")
        XCTAssertEqual(m.burnRate!, 0.18 / 1.5, accuracy: 0.0001, "$ per minute across finished turns")
    }

    func testCompactNumbers() {
        XCTAssertEqual(compact(950), "950")
        XCTAssertEqual(compact(12_400), "12.4k")
        XCTAssertEqual(compact(1_250_000), "1.25M")
    }
}

// MARK: - Subagents and questions

@MainActor
final class SubagentTests: XCTestCase {
    private func model() -> SessionModel { SessionModel(client: Client(port: 0)) }
    private func feed(_ json: String, _ m: SessionModel, _ t: Turn) {
        m.record(Data(json.utf8), into: t)
    }

    /// A subagent's calls nest under the `Task` that started it rather than joining the list.
    func testASubagentsCallsNestUnderItsTask() {
        let m = model(), t = Turn(prompt: "hi")
        feed(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"task1","name":"Task","input":{"description":"survey auth"}}]}}"#, m, t)
        feed(#"{"type":"assistant","parent_tool_use_id":"task1","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"src/auth.ts"}}]}}"#, m, t)
        feed(#"{"type":"assistant","parent_tool_use_id":"task1","message":{"content":[{"type":"tool_use","id":"w1","name":"Write","input":{"file_path":"src/new.ts"}}]}}"#, m, t)

        XCTAssertEqual(t.calls.count, 1, "the child calls are not top-level rows")
        XCTAssertEqual(t.calls[0].children.map(\.tool), ["Read", "Write"])
        XCTAssertEqual(t.files, ["src/new.ts"], "a file the subagent wrote is the turn's file")

        // The rail names what is happening now, not the Task it is happening inside.
        m.turns = [t]; m.running = true
        if case .working(let what) = m.activity {
            XCTAssertEqual(what, "Write src/new.ts")
        } else { XCTFail("running") }

        feed(#"{"type":"user","parent_tool_use_id":"task1","message":{"content":[{"type":"tool_result","tool_use_id":"w1","content":"ok"}]}}"#, m, t)
        XCTAssertFalse(t.calls[0].children[1].running, "a child's result finds the child")
        XCTAssertTrue(t.calls[0].running, "the Task itself is still going")
    }

    /// A question's options survive decoding, and the answer body carries them as text.
    func testAQuestionDecodesItsOptions() throws {
        let json = #"{"id":"q1","tool":"AskUserQuestion","command":"","rules":[],"session_id":"s","input":{"questions":[{"question":"Which DB?","header":"DB","multiSelect":false,"options":[{"label":"Postgres","description":"x"},{"label":"D1","description":"y"}]}]}}"#
        let p = try JSONDecoder().decode(Wire.Pending.self, from: Data(json.utf8))
        XCTAssertTrue(p.isQuestion)
        XCTAssertEqual(p.questions.map(\.text), ["Which DB?"])
        XCTAssertEqual(p.questions[0].options.map(\.label), ["Postgres", "D1"])
        // The description says what choosing it means, and it used to be dropped at the parse —
        // leaving a row of one-word options to guess between.
        XCTAssertEqual(p.questions[0].options.map(\.detail), ["x", "y"])
        XCTAssertFalse(p.questions[0].multiSelect)
    }
}


// MARK: - Diff marks and the palette

@MainActor
final class ReviewUXTests: XCTestCase {
    /// The changed span of a changed line, not the whole line.
    func testTheChangedSpanIsMarked() {
        let (a, b) = Intraline.span("let x = foo(1)", "let x = bar(1)")
        XCTAssertEqual(a, 8..<11)
        XCTAssertEqual(b, 8..<11)
        // Entirely different lines get no mark: all of it is none of it.
        XCTAssertNil(Intraline.span("abc", "xyz").0)
        // An insertion marks nothing on the old side and the inserted text on the new.
        let (c, d) = Intraline.span("ab", "aXb")
        XCTAssertNil(c); XCTAssertEqual(d, 1..<2)
    }

    func testMarksPairDeletionsWithAdditionsInOrder() {
        let lines = [
            Wire.DiffLine(kind: "ctx", old: 1, new: 1, text: "same"),
            Wire.DiffLine(kind: "del", old: 2, new: nil, text: "a = 1"),
            Wire.DiffLine(kind: "del", old: 3, new: nil, text: "b = 2"),
            Wire.DiffLine(kind: "add", old: nil, new: 2, text: "a = 9"),
            Wire.DiffLine(kind: "add", old: nil, new: 3, text: "b = 2 // c"),
            Wire.DiffLine(kind: "add", old: nil, new: 4, text: "unpaired"),
        ]
        let m = Intraline.marks(lines)
        XCTAssertNil(m[0])
        XCTAssertEqual(m[1], 4..<5); XCTAssertEqual(m[3], 4..<5)
        XCTAssertNil(m[2], "nothing removed from the old line")
        XCTAssertEqual(m[4], 5..<10)
        XCTAssertNil(m[5], "the unpaired tail is whole")
    }

    /// `nlb` finds the lane verb; a word-start run beats scattered letters; a miss is nil.
    func testFuzzyFindsSubsequencesAndRanksWordStarts() {
        XCTAssertNotNil(Fuzzy.score("nlb", in: "New lane on its own branch"))
        XCTAssertNil(Fuzzy.score("xyz", in: "New lane on its own branch"))
        let verb = Fuzzy.score("stop", in: "Stop the turn")!
        let file = Fuzzy.score("stop", in: "src/components/StopButtonWrapper.tsx")!
        XCTAssertGreaterThan(verb, file, "the exact verb outranks the file that contains it")
    }

    func testRecentsAreRememberedMostRecentFirst() {
        UserDefaults.standard.removeObject(forKey: "keel.palette.recent")
        Recent.remember("a"); Recent.remember("b"); Recent.remember("a")
        XCTAssertEqual(Recent.titles, ["a", "b"])
        UserDefaults.standard.removeObject(forKey: "keel.palette.recent")
    }
}


// MARK: - Long text in the box

@MainActor
final class LongTextTests: XCTestCase {
    func testALongBlockLeavesTheBoxAsAChipWithASummary() {
        let m = SessionModel(client: Client(port: 0))
        let text = "\n  <html>\n" + String(repeating: "<div>row</div>\n", count: 400)
        m.prompt = text
        m.fileLongText()
        XCTAssertEqual(m.prompt, "", "the box is clear again")
        XCTAssertEqual(SessionModel.summary(of: text), "<html>", "the first non-empty line")
        XCTAssertEqual(SessionModel.summary(of: String(repeating: "x", count: 100)).count, 48)
        // Short text stays where it was typed.
        m.prompt = "just a sentence"
        m.fileLongText()
        XCTAssertEqual(m.prompt, "just a sentence")
    }
}

// MARK: - Paragraphs

@MainActor
final class ParagraphTests: XCTestCase {
    private func model() -> SessionModel { SessionModel(client: Client(port: 0)) }
    private func feed(_ json: String, _ m: SessionModel, _ t: Turn) { m.record(Data(json.utf8), into: t) }

    /// A second text block — after a tool call — starts a new paragraph, not a new word on the
    /// end of the old one.
    func testANewTextBlockIsANewParagraph() {
        let m = model(), t = Turn(prompt: "hi")
        feed(#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text"}}}"#, m, t)
        feed(#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Let me look."}}}"#, m, t)
        feed(#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use"}}}"#, m, t)
        feed(#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text"}}}"#, m, t)
        feed(#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Done."}}}"#, m, t)
        XCTAssertEqual(t.text, "Let me look.\n\nDone.")
    }

    /// Without partial messages, the whole assistant message is the only copy of the prose.
    func testWholeMessagesAreTakenWhenNothingStreamed() {
        let m = model(), t = Turn(prompt: "hi")
        feed(#"{"type":"assistant","message":{"content":[{"type":"text","text":"First."}]}}"#, m, t)
        feed(#"{"type":"assistant","message":{"content":[{"type":"text","text":"Second."}]}}"#, m, t)
        XCTAssertEqual(t.text, "First.\n\nSecond.")
        // But when deltas streamed it, the whole message is not appended a second time.
        let t2 = Turn(prompt: "hi")
        feed(#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Streamed."}}}"#, m, t2)
        feed(#"{"type":"assistant","message":{"content":[{"type":"text","text":"Streamed."}]}}"#, m, t2)
        XCTAssertEqual(t2.text, "Streamed.")
    }
}
