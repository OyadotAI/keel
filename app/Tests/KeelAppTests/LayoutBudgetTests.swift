import AppKit
import SwiftUI
import XCTest
@testable import KeelApp

/// What one layout pass of a pane costs, at the moment something is animating over it.
///
/// Nothing lays a pane out once. A card arriving runs `K.M.enter` — the app's one spring, and a
/// spring settles well past its response — and every frame of it proposes the transcript a new
/// height. So the number that matters is not the cost of a pane at rest but the cost of a pane
/// being re-proposed, times a hundred and twenty frames, on the main thread. Past two seconds of
/// that the run loop is late enough that macOS calls it a hang, and the stack it reports names
/// `StackLayout.placeChildren` and an `NSTextField` and nothing of Keel's at all.
///
/// Kept as a budget because both halves of that product are easy to make worse by accident: an
/// `.animation(_:value:)` moved up a level puts the whole subtree back inside the loop, and a
/// leaf that measures its own text puts AppKit back inside every frame.
@MainActor
final class LayoutBudgetTests: XCTestCase {
    private func session(turns n: Int) -> SessionModel {
        let m = SessionModel(client: Client(port: 0))
        m.loaded = true
        var out: [Turn] = []
        for k in 0..<n {
            let t = Turn(prompt: "ask number \(k)")
            for i in 0..<14 {
                let id = "c-\(k)-\(i)"
                m.record(Data(#"""
                {"type":"assistant","timestamp":"2026-01-01T10:00:0\#(i % 10)Z","message":{
                  "usage":{"input_tokens":10,"output_tokens":5},
                  "content":[{"type":"text","text":"A paragraph of reply, with `code` and **bold**.\n\n- a bullet\n- another"},
                             {"type":"tool_use","id":"\#(id)","name":"Bash","input":{"command":"rg -n pattern src/"}}]}}
                """#.utf8), into: t)
                let output = String(repeating: "a line of tool output\n", count: 200)
                m.record(Data(#"""
                {"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"\#(id)",
                 "content":\#(String(decoding: try! JSONEncoder().encode(output), as: UTF8.self))}]}}
                """#.utf8), into: t)
            }
            t.settle(); t.finished = true
            out.append(t)
        }
        m.turns = out
        return m
    }

    /// The card the hang reports were taken during: it is what the `MoveTransition` in those
    /// stacks is, and its own `TextField` is what AppKit was measuring at the leaf.
    private func question() -> Wire.Pending {
        let json = """
        {"id":"q1","tool":"AskUserQuestion","command":"","rules":[],"session_id":"s",
         "input":{"questions":[{"question":"Which way should this go?","multiSelect":false,
          "options":[{"label":"The first way","description":"and what it means"},
                     {"label":"The second way","description":"and what that means"}]}]}}
        """
        return try! JSONDecoder().decode(Wire.Pending.self, from: Data(json.utf8))
    }

    /// Re-proposed a new height each pass, which is what an entering card does to its siblings.
    @discardableResult
    private func time<V: View>(_ label: String, _ view: V, passes: Int = 10) -> TimeInterval {
        let host = NSHostingView(rootView: view)
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 900, height: 700),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.contentView = host
        window.orderFront(nil)
        host.layoutSubtreeIfNeeded()
        host.displayIfNeeded()
        let started = Date()
        for i in 0..<passes {
            window.setContentSize(CGSize(width: 900, height: 700 - CGFloat(i % 30)))
            host.needsLayout = true
            host.layoutSubtreeIfNeeded()
            host.display()
        }
        let each = Date().timeIntervalSince(started) / Double(passes)
        print("layout: \(label) in \(Int(each * 1000))ms/pass")
        window.orderOut(nil)
        return each
    }

    func testAPaneLaysOutInsideAFrame() {
        let m = session(turns: 12)
        m.pending = [question()]
        // A full composer as well. A vertical-axis `TextField` is an `NSTextField` measured
        // through AppKit, and it measured its whole 1,500 characters on every proposal until it
        // was given a fixed vertical size: 13 ms a pass, against 8 ms with one.
        m.prompt = String(repeating: "a long prompt that wraps over many lines ", count: 36)
        XCTAssertLessThan(time("ChatRail, card entering", ChatRail(model: m)), 0.060)
        XCTAssertLessThan(time("TurnStage", TurnStage(model: m)), 0.060)
    }
}
