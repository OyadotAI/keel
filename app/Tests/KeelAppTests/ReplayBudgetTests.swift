import XCTest
@testable import KeelApp

/// What opening a session costs.
///
/// "Clicking a session loads all the events" was a real measurement, not a feeling: every record
/// appended to `turns`, and each append re-laid the transcript out, re-parsed a markdown block and
/// rebuilt every group with a deep copy of every byte the turn had printed. This pins the decode
/// cost so the next version of that regression fails here rather than in front of somebody.
@MainActor
final class ReplayBudgetTests: XCTestCase {

    /// A session the size of this repository's own: 600 records, 170 calls with real output.
    private func transcript() -> [Data] {
        var out: [Data] = []
        for turn in 0..<12 {
            out.append(Data(#"{"type":"user","message":{"content":"ask number \#(turn)"}}"#.utf8))
            for i in 0..<14 {
                let id = "c-\(turn)-\(i)"
                out.append(Data(#"""
                {"type":"assistant","timestamp":"2026-01-01T10:00:0\#(i % 10)Z","message":{
                  "usage":{"input_tokens":10,"output_tokens":5},
                  "content":[{"type":"text","text":"A paragraph of reply, with `code` and **bold**.\n\n- a bullet\n- another"},
                             {"type":"tool_use","id":"\#(id)","name":"Bash","input":{"command":"rg -n pattern src/"}}]}}
                """#.utf8))
                let output = String(repeating: "a line of tool output\n", count: 200)
                out.append(Data(#"""
                {"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"\#(id)",
                 "content":\#(String(decoding: try! JSONEncoder().encode(output), as: UTF8.self))}]}}
                """#.utf8))
            }
        }
        return out
    }

    func testDecodingAWholeSessionIsFast() {
        let records = transcript()
        XCTAssertGreaterThan(records.count, 300, "a realistic session")

        let m = SessionModel(client: Client(port: 0))
        var turns: [Turn] = []
        var live: Turn?

        let started = Date()
        for data in records {
            if let asked = SessionModel.asked(in: data) {
                let t = Turn(prompt: asked); t.replayed = true
                turns.append(t); live = t
            } else if let t = live {
                m.record(data, into: t)
            }
        }
        turns.forEach { $0.settle() }
        let took = Date().timeIntervalSince(started)

        print("replay: \(records.count) records, \(turns.count) turns, "
              + "\(turns.reduce(0) { $0 + $1.calls.count }) calls in "
              + "\(Int(took * 1000))ms")

        XCTAssertEqual(turns.count, 12)
        XCTAssertEqual(turns.reduce(0) { $0 + $1.calls.count }, 168)
        XCTAssertTrue(turns.allSatisfy { $0.groups.count == $0.calls.count / 14 || !$0.groups.isEmpty },
                      "groups were maintained, not left empty")
        // Generous against a debug build on a loaded machine, and still an order of magnitude
        // below what the per-record rebuild cost.
        XCTAssertLessThan(took, 1.5, "opening a session must not be a visible stall")
    }

    /// What a long reply costs on the way in.
    ///
    /// The parse is O(n) in the block, so running it per token is O(n²) in the reply — and both
    /// earlier versions did exactly that, once over the whole turn and once over the current
    /// block. A ten-kilobyte answer is a normal thing for an agent to write; it must not be a
    /// quadratic thing for Keel to receive.
    func testALongReplyStreamsInWithoutQuadraticParsing() {
        let m = SessionModel(client: Client(port: 0))
        let t = Turn(prompt: "explain it")
        m.record(Data(#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text"}}}"#.utf8), into: t)

        // Roughly ten kilobytes, arriving the way a reply does.
        let deltas = (0..<2_000).map { i in
            Data(#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"word\#(i % 10) "}}}"#.utf8)
        }
        let started = Date()
        for d in deltas { m.record(d, into: t) }
        t.settle()
        let took = Date().timeIntervalSince(started)
        print("stream: \(deltas.count) deltas, \(t.text.count) chars in \(Int(took * 1000))ms")

        XCTAssertGreaterThan(t.text.count, 10_000)
        XCTAssertEqual(t.steps.count, 1, "one block, however many deltas built it")
        // Per-token parsing put this in the seconds; the ceiling is deliberately far above the
        // measurement so it fails on a change of shape, not on a busy machine.
        XCTAssertLessThan(took, 0.5, "a reply must not cost more than linear to receive")
    }
}

/// What a diff card is allowed to lay out.
///
/// A card's stack is not lazy, so every row it is handed is instantiated and sized on the main
/// thread. The daemon caps a diff at 3,000 lines; 3,000 rows is a two-second hang that reports
/// itself as `StackLayout.placeChildren` with no Keel frame anywhere in it.
final class DiffPreviewTests: XCTestCase {
    private func hunk(_ n: Int) -> Wire.Hunk {
        Wire.Hunk(header: "@@ -1 +1 @@",
                  lines: (0..<n).map { Wire.DiffLine(kind: "add", old: nil, new: $0, text: "line \($0)") })
    }

    func testACardStopsAtItsCeiling() {
        // A new file arrives as one hunk holding all of it, so the cut has to land inside a hunk.
        let one = Wire.Diff(path: "big.lock", hunks: [hunk(3_000)], untracked: true, note: nil)
        let (shown, dropped) = one.preview(200)
        XCTAssertEqual(shown.reduce(0) { $0 + $1.lines.count }, 200)
        XCTAssertEqual(dropped, 2_800)

        // Across hunks, the ones past the ceiling are not drawn at all.
        let many = Wire.Diff(path: "a.swift", hunks: [hunk(150), hunk(150), hunk(150)],
                             untracked: false, note: nil)
        let (kept, left) = many.preview(200)
        XCTAssertEqual(kept.count, 2)
        XCTAssertEqual(kept.reduce(0) { $0 + $1.lines.count }, 200)
        XCTAssertEqual(left, 250)

        // A small diff is untouched and says so.
        let small = Wire.Diff(path: "a.swift", hunks: [hunk(9)], untracked: false, note: nil)
        XCTAssertEqual(small.preview(200).dropped, 0)
        XCTAssertEqual(small.preview(200).hunks.first?.lines.count, 9)
    }
}
