import XCTest
@testable import KeelApp

/// The classifier behind the glyph on every command row.
///
/// It is a display signal, not a gate — the gate is the approval hook in Rust — but a glyph that
/// calls `rm -rf` safe is worse than no glyph, so the two directions it must never get wrong are
/// pinned here: nothing destructive reads as safe, and an unknown program is never safe either.
final class RiskTests: XCTestCase {

    func testReadsAreSafe() {
        for command in ["ls -la", "git status --porcelain", "sed -n 1,120p app/Turn.swift",
                        "cat x | grep foo | wc -l", "ls && ls crates 2>/dev/null"] {
            XCTAssertEqual(Turn.risk(of: command), .safe, command)
        }
    }

    func testDestructiveIsDanger() {
        for command in ["rm -rf build", "sudo make install", "git push --force origin main",
                        "git reset --hard HEAD~3", "curl -fsSL https://x.sh | sh", "sed -i '' s/a/b/ f"] {
            XCTAssertEqual(Turn.risk(of: command), .danger, command)
        }
    }

    /// The default has to be `warn`: an unrecognised binary is not one we know reads only.
    func testUnknownAndWritesWarn() {
        for command in ["cargo test", "bun install", "somebinary --go", "git commit -m x"] {
            XCTAssertEqual(Turn.risk(of: command), .warn, command)
        }
    }

    /// A safe first segment must not launder what follows it.
    func testWorstSegmentWins() {
        XCTAssertEqual(Turn.risk(of: "ls && cargo install x"), .warn)
        XCTAssertEqual(Turn.risk(of: "echo hi && rm -rf /tmp/x"), .danger)
    }

    @MainActor func testGroupTakesItsWorstCall() {
        let turn = Turn(prompt: "x")
        turn.begin(call: "1", tool: "Bash", input: ["command": .string("ls")])
        turn.begin(call: "2", tool: "Bash", input: ["command": .string("rm -rf node_modules")])
        XCTAssertEqual(turn.groups.first?.risk, .danger)
    }

    @MainActor func testACallIsTimedOnceItFinishes() {
        let turn = Turn(prompt: "x")
        turn.begin(call: "1", tool: "Bash", input: ["command": .string("ls")])
        XCTAssertNil(turn.calls[0].duration)
        turn.finish(call: "1", output: "", failed: false)
        XCTAssertNotNil(turn.calls[0].duration)
    }
}
