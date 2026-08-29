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

    func testCommentsAndKeepAlivesAreNotEvents() {
        XCTAssertTrue(parse(":keep-alive\nid: 7\nretry: 100").isEmpty)
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

/// The Markdown parse, which runs on every frame of a streaming reply.
@MainActor
final class MarkdownTests: XCTestCase {

    /// The same text must not be re-parsed. `body` runs per text delta, so a long answer was being
    /// parsed thousands of times on the way in — the pane stuttered under a scroll because of it.
    func testTheSameSourceIsParsedOnce() {
        let text = "# Title\n\nSome body.\n\n- one\n- two"
        let first = Markdown.cachedBlocks(text)
        let second = Markdown.cachedBlocks(text)
        XCTAssertEqual(first.count, second.count)
        XCTAssertEqual(first.count, 3, "heading, paragraph, bullets")
    }

    /// Several turns render in one pass, so a one-slot cache would be evicted by its neighbour
    /// every time and buy nothing.
    func testDifferentSourcesBothStayCached() {
        let a = "First answer"
        let b = "Second answer"
        _ = Markdown.cachedBlocks(a)
        _ = Markdown.cachedBlocks(b)
        XCTAssertEqual(Markdown.cachedBlocks(a).count, 1)
        XCTAssertEqual(Markdown.cachedBlocks(b).count, 1)
    }

    /// A streaming reply produces one new string per delta, so the cache has to be bounded or it
    /// holds every intermediate state of every answer for the life of the app.
    func testTheCacheIsBounded() {
        for i in 0..<200 { _ = Markdown.cachedBlocks("delta \(i)") }
        // Nothing to assert but that it still answers correctly after eviction.
        XCTAssertEqual(Markdown.cachedBlocks("delta 199").count, 1)
        XCTAssertEqual(Markdown.cachedBlocks("delta 0").count, 1)
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
