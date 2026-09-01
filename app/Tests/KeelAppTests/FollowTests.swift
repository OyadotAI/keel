import AppKit
import SwiftUI
import XCTest
@testable import KeelApp

/// The follow path — a session running in a terminal, watched from Keel — through `apply`, with
/// no socket. The events are built here and handed to the same consumer the stream feeds, which
/// is the point: there is one consumer, and these pin what it does with a turn it does not own.
///
/// Nothing tested `follow` before this. The replay coverage was at the decoder (`StreamTests`)
/// and the budget (`ReplayBudgetTests`); the state machine around the decoder — what is
/// `running`, what is `finished`, what is on screen while the read fails — was covered by nobody,
/// and it is where every "the trace shows the wrong thing" report came from.
@MainActor
final class FollowTests: XCTestCase {
    private func model(live: Bool = true, busy: Bool = true) -> SessionModel {
        let m = SessionModel(client: Client(port: 0))
        m.loaded = true
        m.sessionId = "abc-1"
        m.sessions = [Wire.Session(id: "abc-1", title: "t", messages: 1, lastActive: nil,
                                   scope: "here", cwd: nil, live: live, busy: busy)]
        // What `open(session:)` sets before the stream starts.
        m.replaying = true
        return m
    }

    private func user(_ text: String, uuid: String = "u-1",
                      at: String = "2026-09-01T10:00:00.000Z") -> Client.Event {
        Client.Event(name: "msg", data: #"{"type":"user","uuid":"\#(uuid)","timestamp":"\#(at)","message":{"role":"user","content":"\#(text)"}}"#)
    }

    private func fact(_ json: String) -> Client.Event { Client.Event(name: "fact", data: json) }

    private func assistant(_ text: String, stop: String,
                           at: String = "2026-09-01T10:00:05.000Z") -> Client.Event {
        Client.Event(name: "msg", data: #"{"type":"assistant","timestamp":"\#(at)","message":{"role":"assistant","stop_reason":"\#(stop)","content":[{"type":"text","text":"\#(text)"}],"usage":{"input_tokens":10,"output_tokens":5}}}"#)
    }

    private func toolCall(at: String = "2026-09-01T10:00:02.000Z") -> Client.Event {
        Client.Event(name: "msg", data: #"{"type":"assistant","timestamp":"\#(at)","message":{"role":"assistant","stop_reason":"tool_use","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#)
    }

    private let caughtUp = Client.Event(name: "caught-up", data: "0")

    private func layout<V: View>(_ view: V) {
        let host = NSHostingView(rootView: view)
        for size in [CGSize.zero, CGSize(width: 2, height: 400), CGSize(width: 900, height: 600)] {
            host.frame = CGRect(origin: .zero, size: size)
            host.layoutSubtreeIfNeeded()
            host.layout()
        }
    }

    /// Nothing reaches `turns` until the daemon says it has caught up, and everything that does
    /// is foreign and over.
    ///
    /// Rejected: asserting on `replaying` alone. It was cleared by two different exits, and a
    /// test of the flag passed while the turns it was meant to guard were still being appended
    /// one at a time.
    func testCatchUpHandsOverOnceAndEveryTurnIsForeign() async {
        let m = model(busy: false)
        for i in 1...3 {
            await m.apply(user("ask \(i)"))
            await m.apply(assistant("answer \(i)", stop: "end_turn"))
        }
        XCTAssertEqual(m.turns.count, 0, "held back until caught-up")
        XCTAssertEqual(m.replay, .reading)
        await m.apply(caughtUp)
        XCTAssertEqual(m.turns.count, 3)
        XCTAssertTrue(m.turns.allSatisfy(\.replayed))
        XCTAssertTrue(m.turns.allSatisfy(\.finished))
        XCTAssertEqual(m.replay, .none)
        XCTAssertTrue(m.following)
        XCTAssertFalse(m.running)
    }

    /// A turn arriving after the catch-up is one running right now: the working bar shows, and
    /// the transcript's own `end_turn` ends it — Keel is not there to.
    func testAFollowedTurnRunsThenFinishesOnItsOwnRecord() async {
        let m = model(live: true, busy: true)
        await m.apply(user("first"))
        await m.apply(toolCall())
        await m.apply(caughtUp)
        XCTAssertTrue(m.running, "busy at caught-up is a turn in flight")
        XCTAssertFalse(m.turns.last!.finished)
        m.editing = "src/App.tsx"

        await m.apply(assistant("done", stop: "end_turn"))
        XCTAssertFalse(m.running)
        XCTAssertTrue(m.turns.last!.finished)
        XCTAssertNil(m.editing, "a followed turn used to leak this for the life of the lane")

        await m.apply(user("second"))
        XCTAssertEqual(m.turns.count, 2)
        XCTAssertTrue(m.turns[0].finished)
        XCTAssertTrue(m.running, "the next prompt opens the next turn")
        XCTAssertFalse(m.turns[1].finished)
    }

    /// A followed turn did work, and none of it is this lane's to judge.
    func testAFollowedTurnIsNotTheLanesVerdict() async {
        let m = model(busy: false)
        await m.apply(user("do it"))
        await m.apply(toolCall())
        await m.apply(assistant("did it", stop: "end_turn"))
        await m.apply(caughtUp)
        XCTAssertTrue(m.turns.last!.didWork)
        XCTAssertTrue(m.turns.last!.replayed)
        if case .notRun = m.latestGate {} else { XCTFail("no gate was run, none should be reported") }
        XCTAssertEqual(m.mergeBlocker, "Nothing has been changed yet.")
    }

    /// A tail that fails before the conversation is on screen keeps what was on screen and says
    /// why in the pane — not as a banner beside an empty one.
    func testATailThatFailsBeforeCatchUpKeepsWhatWasOnScreen() async {
        let m = model()
        let before = Turn(prompt: "what was there")
        before.finished = true
        m.turns = [before]
        await m.apply(user("new"))
        await m.apply(Client.Event(name: "fatal", data: "that session's transcript is not on this machine"))
        XCTAssertEqual(m.replay, .failed("that session's transcript is not on this machine"))
        XCTAssertEqual(m.turns.count, 1)
        XCTAssertTrue(m.turns[0] === before)
        XCTAssertNil(m.lastError)
        layout(TurnStage(model: m))
        layout(ChatRail(model: m))
    }

    /// The daemon dropped the head of a long session; both panes say so.
    func testTruncationIsSaidInBothPanes() async {
        let m = model(busy: false)
        await m.apply(Client.Event(name: "truncated", data: "1200"))
        await m.apply(user("late"))
        await m.apply(assistant("ok", stop: "end_turn"))
        await m.apply(caughtUp)
        XCTAssertEqual(m.replayDropped, 1200)
        XCTAssertEqual(SessionModel.droppedNotice(1200),
                       "1200 earlier records were not loaded — this session is longer than Keel replays.")
        XCTAssertNil(SessionModel.droppedNotice(0))
        layout(TurnStage(model: m))
        layout(ChatRail(model: m))
    }

    /// The same records through the owned path and the followed one make the same turn. This is
    /// the pin for "everything that shows up live shows up on replay".
    func testLiveAndFollowedFeedsProduceTheSameTurn() async {
        let live = model()
        live.owned = true
        live.turns = [Turn(prompt: "ask")]
        let seen = model(busy: false)
        await seen.apply(user("ask"))
        for event in [toolCall(), assistant("reply", stop: "end_turn")] {
            await live.apply(event)
            await seen.apply(event)
        }
        await seen.apply(caughtUp)
        let a = live.turns[0], b = seen.turns[0]
        XCTAssertEqual(a.text, b.text)
        XCTAssertEqual(a.calls.map(\.tool), b.calls.map(\.tool))
        XCTAssertEqual(a.tokens, b.tokens)
        XCTAssertEqual(a.files, b.files)
        XCTAssertEqual(a.failed, b.failed)
        // What legitimately differs: whose it is, and how long it took — a live turn is timed by
        // its `result` record, a followed one by the span from the prompt to the last reply.
        XCTAssertFalse(a.replayed)
        XCTAssertTrue(b.replayed)
        XCTAssertNotNil(b.durationMS)
    }

    /// A replayed turn's clock is the transcript's, not the moment it was parsed.
    func testAReplayedTurnStartsWhenItsFirstRecordWas() async {
        let m = model(busy: false)
        await m.apply(user("ask", at: "2026-08-30T09:00:00.000Z"))
        await m.apply(assistant("ok", stop: "end_turn", at: "2026-08-30T09:00:04.000Z"))
        await m.apply(caughtUp)
        let started = m.turns[0].started
        XCTAssertEqual(started.timeIntervalSince1970,
                       SessionModel.moment("2026-08-30T09:00:00.000Z")!.timeIntervalSince1970,
                       accuracy: 0.001)
    }

    /// A followed stream that goes quiet for a minute is a dead daemon, and the pane says so
    /// instead of "Following this session" for ever.
    func testTheDeadStreamWatchdogEndsAFollow() async {
        let m = model(live: true, busy: true)
        await m.apply(user("ask"))
        await m.apply(caughtUp)
        XCTAssertTrue(m.running)
        m.lastEventAt = Date().addingTimeInterval(-120)
        m.checkPulse()
        XCTAssertEqual(m.replay, .failed("The connection to Keel's daemon stopped responding."))
        XCTAssertFalse(m.following)
        XCTAssertFalse(m.running)
        XCTAssertTrue(m.turns.last!.finished)
    }

    /// The pulse does nothing for a turn this lane owns: the chat stream has its own check.
    func testThePulseLeavesAnOwnedTurnAlone() {
        let m = model()
        m.owned = true
        m.replay = .none
        m.lastEventAt = Date().addingTimeInterval(-120)
        m.checkPulse()
        XCTAssertEqual(m.replay, .none)
    }

    /// A fact names a turn by the record that opened it, and lands nowhere else.
    func testAFactLandsOnlyOnTheTurnItNames() async {
        let m = model(busy: false)
        await m.apply(user("first", uuid: "u-1"))
        await m.apply(assistant("one", stop: "end_turn"))
        await m.apply(fact(#"{"turn":"u-1","kind":"turn.commit","sha":"abc123"}"#))
        await m.apply(user("second", uuid: "u-2"))
        await m.apply(fact(#"{"turn":"u-9","kind":"turn.commit","sha":"nobody"}"#))
        await m.apply(fact(#"{"turn":null,"kind":"turn.commit","sha":"keyless"}"#))
        await m.apply(assistant("two", stop: "end_turn"))
        await m.apply(caughtUp)
        XCTAssertEqual(m.turns[0].key, "u-1")
        XCTAssertEqual(m.turns[0].commit, "abc123")
        XCTAssertNil(m.turns[1].commit, "a fact for a turn nobody has, or for no turn, is dropped")
    }

    /// A fact fills what the turn does not know and never overwrites what this lane measured.
    func testFactsFillButNeverOverwrite() async {
        let live = model()
        live.owned = true
        let t = Turn(prompt: "ask")
        t.cost = 0.5
        live.turns = [t]
        live.fact(#"{"turn":"u-1","kind":"turn.usage","input":10,"output":5,"cache_read":0,"cache_write":0,"cost_usd":0.9}"#)
        XCTAssertEqual(t.cost, 0.5, "the result record already said")
        XCTAssertEqual(t.tokens?.input, 10, "what it did not have, it takes")

        let seen = model(busy: false)
        await seen.apply(user("ask", uuid: "u-1"))
        await seen.apply(assistant("ok", stop: "end_turn"))
        await seen.apply(fact(#"{"turn":"u-1","kind":"turn.usage","input":10,"output":5,"cache_read":0,"cache_write":0,"cost_usd":0.9}"#))
        await seen.apply(caughtUp)
        XCTAssertEqual(seen.turns[0].cost, 0.9, "a replayed turn's cost is the provider's total")
        XCTAssertEqual(seen.turns[0].tokens?.input, 10, "and its tokens, which were a sum of records")
    }

    /// The gate comes back whole: its spinner while it runs, its problems, and its duration.
    func testGateProblemsAndDurationSurviveAReopen() async {
        let m = model(busy: false)
        await m.apply(user("build", uuid: "u-1"))
        await m.apply(assistant("ok", stop: "end_turn"))
        await m.apply(caughtUp)
        await m.apply(fact(#"{"turn":"u-1","kind":"turn.gate","status":"running","command":"make check"}"#))
        XCTAssertEqual(m.turns[0].gate, .running("make check"))
        await m.apply(fact(#"{"turn":"u-1","kind":"turn.gate","status":"failed","command":"make check","ms":4100,"problems":[{"file":"src/a.rs","line":3,"col":1,"severity":"error","message":"boom"}]}"#))
        guard case .failed(let cmd, let problems) = m.turns[0].gate else { return XCTFail("no verdict") }
        XCTAssertEqual(cmd, "make check")
        XCTAssertEqual(problems.map(\.file), ["src/a.rs"])
        await m.apply(fact(#"{"turn":"u-1","kind":"turn.gate","status":"passed","command":"make check","ms":4100}"#))
        XCTAssertEqual(m.turns[0].gate, .passed("make check", 4.1))
    }

    /// The Trace at each of its empty states, in the geometry that has crashed before.
    func testTheTraceDrawsEveryEmptyState() {
        let m = model()
        m.turns = []
        m.replay = .reading
        layout(TurnStage(model: m))
        m.replay = .failed("no such transcript")
        layout(TurnStage(model: m))
        m.replay = .none
        layout(TurnStage(model: m))
    }
}
