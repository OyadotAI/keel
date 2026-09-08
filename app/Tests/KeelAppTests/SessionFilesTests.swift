import Foundation
import XCTest
@testable import KeelApp

@MainActor
final class SessionFilesTests: XCTestCase {
    private func model() -> SessionModel {
        let model = SessionModel(client: Client(port: 0))
        model.repoPath = "/test/session-files"
        model.isRepo = true
        return model
    }

    func testHistoricalFilesReportedByDaemonAppearWithoutEditCalls() {
        let model = model()
        let turn = Turn(prompt: "Update the project")
        turn.replayed = true
        turn.key = "u-1"
        model.turns = [turn]
        model.owned = false
        model.fact(#"{"turn":"u-1","kind":"turn.files","at":"","files":["src/a.swift","src/b.swift"]}"#)
        XCTAssertEqual(model.editedThisSession.map(\.path), ["src/a.swift", "src/b.swift"])
    }

    func testSubagentWritesAppearAlongsideTopLevelEdits() {
        let model = model()
        let turn = Turn(prompt: "Delegate the change")
        turn.replayed = true
        turn.begin(call: "task", tool: "Task", input: [:])
        turn.begin(call: "child", tool: "Edit", input: ["file_path": .string("src/child.swift")], parent: "task")
        turn.begin(call: "direct", tool: "Write", input: ["file_path": .string("src/direct.swift")])
        model.turns = [turn]
        XCTAssertEqual(model.editedThisSession.map(\.path), ["src/child.swift", "src/direct.swift"])
    }

    func testHistoricalMissingFilesAndFullLengthPathsAreNotDiscarded() {
        let model = model()
        let turn = Turn(prompt: "An old edit")
        turn.replayed = true
        let longPath = "src/" + String(repeating: "nested/", count: 30) + "file.swift"
        turn.begin(call: "long", tool: "Edit", input: ["file_path": .string(longPath)])
        turn.begin(call: "gone", tool: "Edit", input: ["file_path": .string(model.repoPath + "/gone.swift")])
        model.turns = [turn]
        XCTAssertEqual(model.editedThisSession.map(\.path), [longPath, "gone.swift"])
    }

    func testHistoricalFilesOutsideTheOpenCheckoutAreStillListed() {
        let model = model()
        let turn = Turn(prompt: "Edit a neighbouring project")
        turn.replayed = true
        turn.begin(call: "outside", tool: "Edit", input: ["file_path": .string("/test/other/src/a.swift")])
        model.turns = [turn]
        XCTAssertEqual(model.editedThisSession.map(\.path), ["/test/other/src/a.swift"])
    }

    func testRecordedNotebookPathIsIncluded() {
        let model = model()
        let turn = Turn(prompt: "Update the notebook")
        turn.replayed = true
        turn.begin(call: "notebook", tool: "NotebookEdit", input: ["notebook_path": .string("analysis.ipynb")])
        model.turns = [turn]
        XCTAssertEqual(model.editedThisSession.map(\.path), ["analysis.ipynb"])
    }

    /// These are the command shapes found in a real historical session. Neither current mtime
    /// nor the existence of the old checkout can establish which files that session wrote.
    func testReplayRecoversSuccessfulPythonAndHeredocWriteTargets() throws {
        let model = model()
        let turn = Turn(prompt: "Fix the layout")
        turn.replayed = true
        model.turns = [turn]
        let commands = [
            "python3 - <<'PY'\np='src/a.swift'\ns=open(p).read()\nopen(p,'w').write(s)\nPY",
            "cat >> docs/changes.md <<'DOC'\na note\nDOC",
            "python3 - <<'PY'\nfrom pathlib import Path\np=Path('src/b.swift')\np.write_text('updated')\nPY",
        ]
        for (index, command) in commands.enumerated() {
            let record: [String: Any] = ["type": "assistant", "cwd": model.repoPath,
                "message": ["content": [["type": "tool_use", "id": "c\(index)", "name": "Bash",
                                            "input": ["command": command]]]]]
            model.record(try JSONSerialization.data(withJSONObject: record), into: turn)
            let result: [String: Any] = ["type": "user", "cwd": model.repoPath,
                "message": ["content": [["type": "tool_result", "tool_use_id": "c\(index)", "content": ""]]]]
            model.record(try JSONSerialization.data(withJSONObject: result), into: turn)
        }
        XCTAssertEqual(model.editedThisSession.map(\.path), ["src/a.swift", "docs/changes.md", "src/b.swift"])
    }

    func testReadOnlyAndFailedCommandsDoNotInventHistoricalEdits() {
        let model = model()
        let turn = Turn(prompt: "Inspect the files")
        turn.replayed = true
        model.turns = [turn]
        turn.begin(call: "read", tool: "Bash", input: ["command": .string("python3 - <<'PY'\np='src/a.swift'\nprint(open(p).read())\nPY")])
        turn.finish(call: "read", output: "contents", failed: false)
        turn.begin(call: "failed", tool: "Bash", input: ["command": .string("cat > src/failed.swift <<'END'\ncontents\nEND")])
        turn.finish(call: "failed", output: "Permission denied", failed: true)
        XCTAssertTrue(model.editedThisSession.isEmpty)
    }

    func testRecordedCommandsResolveCdAndQuotedPaths() {
        let command = "cd '/test/other project' && python3 - <<'PY'\nPath('src/my file.swift').write_text('x')\nPY\ncat > notes.md <<'END'\nx\nEND"
        XCTAssertEqual(RecordedFileWrites.paths(in: command, cwd: "/test/original"),
                       ["/test/other project/src/my file.swift", "/test/other project/notes.md"])
    }

    func testRecordedCommandsDoNotParseDocumentBodiesAsWrites() {
        let document = "cat > notes.md <<'END'\ncat > not-a-write.swift\npython3 - <<'PY'\nopen('also-not-a-write', 'w')\nPY\nEND"
        XCTAssertEqual(RecordedFileWrites.paths(in: document, cwd: nil), ["notes.md"])
        let python = "python3 - <<'PY'\ns='''\nopen('not-a-write', 'w')\n'''\nPath('real.swift').write_text(s)\nPY"
        XCTAssertEqual(RecordedFileWrites.paths(in: python, cwd: nil), ["real.swift"])
    }

    func testRecordedCommandsDoNotGuessDynamicTargetsOrReadOnlyPaths() {
        let commands = [
            "python3 - <<'PY'\np='stale.swift'\np=choose_path()\nopen(p, 'w').write('x')\nPY",
            "python3 - <<'PY'\nprint(open('read-only.swift', 'r').read())\nPY",
            "python3 - <<'PY'\nif False:\n    open('not-executed.swift', 'w').write('x')\nPY",
            "python3 - <<'PY'\np='stale.swift'\nif condition:\n    p='different.swift'\nopen(p, 'w').write('x')\nPY",
            "python3 - <<'PY'\nimport os\nos.chdir('/different')\nopen('unknown.swift', 'w').write('x')\nPY",
            "cd \"$PROJECT\" && cat > unknown.swift <<'END'\nx\nEND",
            "cat > /dev/null <<'END'\nx\nEND",
        ]
        for command in commands { XCTAssertEqual(RecordedFileWrites.paths(in: command, cwd: nil), []) }
    }

    func testRecordedWriteRecoveryIsBounded() {
        let tooLarge = "cat > large.swift <<'END'\n" + String(repeating: "x", count: 512 * 1024) + "\nEND"
        XCTAssertTrue(RecordedFileWrites.paths(in: tooLarge, cwd: nil).isEmpty)
        let many = (0..<100).map { "cat > file\($0).swift <<'END'\nx\nEND" }.joined(separator: "\n")
        XCTAssertEqual(RecordedFileWrites.paths(in: many, cwd: nil).count, 40)
    }

    func testDifferentCheckoutsDoNotLoseFilesWithTheSameSuffix() {
        let model = model()
        let turn = Turn(prompt: "Edit both projects")
        turn.replayed = true
        turn.noteEdit("src/a.swift")
        turn.noteEdit("/test/other/src/a.swift")
        turn.noteEdit(model.repoPath + "/src/a.swift")
        model.turns = [turn]
        XCTAssertEqual(model.editedThisSession.map(\.path), ["src/a.swift", "/test/other/src/a.swift"])
    }

    func testHistoryDoesNotClaimThatAbsenceFromGitStatusMeansCommitted() {
        let model = model()
        let turn = Turn(prompt: "An old change")
        turn.replayed = true
        turn.noteEdit("src/a.swift")
        model.turns = [turn]
        XCTAssertEqual(model.editedThisSession.first?.label, "recorded")
        model.changes = [Wire.Change(path: "src/a.swift", status: " D", label: "deleted")]
        XCTAssertEqual(model.editedThisSession.first?.label, "deleted")
    }

    func testRecordedCwdSurvivesMessagesThatOmitIt() throws {
        let model = model()
        let turn = Turn(prompt: "Use this checkout")
        turn.replayed = true
        model.turns = [turn]
        model.record(Data(#"{"type":"system","subtype":"init","cwd":"/test/other"}"#.utf8), into: turn)
        let command = "cat > file.swift <<'END'\nx\nEND"
        let message: [String: Any] = ["type": "assistant", "message": ["content": [
            ["type": "tool_use", "id": "c", "name": "Bash", "input": ["command": command]]]]]
        model.record(try JSONSerialization.data(withJSONObject: message), into: turn)
        turn.finish(call: "c", output: "", failed: false)
        XCTAssertEqual(model.editedThisSession.map(\.path), ["/test/other/file.swift"])
    }

    /// Opt-in verification against the reporter's local transcript. No personal transcript or
    /// command body is copied into repository fixtures, and no recorded command is executed.
    func testSavedHistoryWhenRequested() throws {
        let env = ProcessInfo.processInfo.environment
        guard let path = env["KEEL_HISTORY_REPLAY"], let root = env["KEEL_HISTORY_ROOT"] else {
            throw XCTSkip("Set KEEL_HISTORY_REPLAY and KEEL_HISTORY_ROOT to verify a local transcript")
        }
        let model = model()
        model.repoPath = root
        let turn = Turn(prompt: "Local history verification")
        turn.replayed = true
        model.turns = [turn]
        let transcript = try String(contentsOfFile: path, encoding: .utf8)
        for line in transcript.split(separator: "\n") {
            model.record(Data(line.utf8), into: turn)
        }
        let recovered = Set(model.editedThisSession.map(\.path))
        let expected = Set((env["KEEL_HISTORY_EXPECT_FILES"] ?? "").split(separator: ",").map(String.init))
        XCTAssertFalse(expected.isEmpty, "Provide expected paths so this is an actual regression check")
        XCTAssertTrue(expected.isSubset(of: recovered), "Missing: \(expected.subtracting(recovered).sorted())")
        print("history: recovered \(recovered.count) files; verified \(expected.count) expected paths")
    }
}
