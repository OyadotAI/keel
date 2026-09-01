import XCTest
@testable import KeelApp

/// The sidecar that survives a relaunch.
///
/// The bug it exists for: a turn whose every tool call is `Bash` gets its file list from git, at
/// the end of the turn, and git's answer is nowhere in Claude Code's transcript. Reopening the
/// session rebuilt the turn from the transcript and it came back with its words and none of its
/// work — the case reported as "turn 8 had files but they are not shown anymore".
@MainActor
final class TurnRecordTests: XCTestCase {
    /// The wire shape the daemon reads: the session beside a flattened record, not nested under a
    /// `record` key. Asserted because the two halves are written in different languages and
    /// nothing else would notice them disagreeing until a turn quietly stopped being saved.
    func testTheRequestFlattensTheRecordBesideTheSession() throws {
        var record = Wire.TurnRecord(n: 8)
        record.prompt = "check the code"
        record.files = ["app/Sources/KeelApp/Turn.swift", "/tmp/plan.html"]
        record.commit = "9dbac39"
        record.cost = 0.42

        let body = try JSONEncoder().encode(Wire.TurnRecordRequest(session: "abc-123", record: record))
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: body) as? [String: Any])

        XCTAssertEqual(json["session"] as? String, "abc-123")
        XCTAssertEqual(json["n"] as? Int, 8)
        XCTAssertEqual(json["commit"] as? String, "9dbac39")
        XCTAssertEqual((json["files"] as? [String])?.count, 2)
        XCTAssertNil(json["record"], "the record must be flattened, not nested")
    }

    /// A record with only the fields an older Keel wrote still decodes. The store outlives the
    /// version that wrote it, and a missing `cost` must not take a session's history down.
    func testAnOlderRecordStillDecodes() throws {
        let json = Data(#"{"n":2,"files":["a.rs"]}"#.utf8)
        let record = try JSONDecoder().decode(Wire.TurnRecord.self, from: json)
        XCTAssertEqual(record.files, ["a.rs"])
        XCTAssertNil(record.cost)
        XCTAssertNil(record.gate)
        XCTAssertEqual(record.prompt, "")
    }

    /// Round trip through the daemon's own field names — `cache_read`, not `cacheRead`. A rename
    /// on either side silently drops the cache counts, which are what make a cost figure mean
    /// anything.
    func testTokensKeepTheDaemonsFieldNames() throws {
        let json = Data(#"{"n":1,"tokens":{"input":10,"output":2,"cache_read":90,"cache_write":5}}"#.utf8)
        let record = try JSONDecoder().decode(Wire.TurnRecord.self, from: json)
        XCTAssertEqual(record.tokens?.input, 10)
        XCTAssertEqual(record.tokens?.cache_read, 90)
        XCTAssertEqual(record.tokens?.cache_write, 5)
    }

    /// `noteEdit` is what the restore replays through, and it must stay idempotent: a turn that was
    /// followed live already has some of these paths, and the record carries all of them.
    func testRestoringAFileTwiceKeepsOneRow() {
        let turn = Turn(prompt: "write something")
        for path in ["a.rs", "b.rs", "a.rs"] { turn.noteEdit(path) }
        XCTAssertEqual(turn.files, ["a.rs", "b.rs"])
    }
}
