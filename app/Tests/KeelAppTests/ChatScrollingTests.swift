import AppKit
import Observation
import SwiftUI
import XCTest
@testable import KeelApp

@MainActor
final class ChatScrollingTests: XCTestCase {
    /// Observe the actual row body, not a parallel model of its dependencies. A copy button
    /// reading `turn.text` here used to invalidate the entire turn for every unparsed token.
    func testRawTokensDoNotInvalidateTheWholeChatTurn() {
        let model = SessionModel(client: Client(port: 0))
        let turn = Turn(prompt: "Review this project")
        turn.said("Starting the review.")
        turn.settle()
        let row = ChatTurn(turn: turn, number: 1, model: model)
        let invalidations = Invalidations()

        for _ in 0..<200 {
            withObservationTracking {
                _ = row.body
            } onChange: {
                invalidations.record()
            }
            turn.said(" another token")
        }

        XCTAssertEqual(invalidations.count, 0,
                       "Raw tokens must not rebuild the prompt, every step, and the turn layout")
    }

    /// Positive control: isolating text must not hide newly arriving blocks or tool calls.
    func testNewStepsStillInvalidateTheChatTurn() {
        let model = SessionModel(client: Client(port: 0))
        let turn = Turn(prompt: "Review this project")
        let row = ChatTurn(turn: turn, number: 1, model: model)
        let invalidations = Invalidations()
        withObservationTracking {
            _ = row.body
        } onChange: {
            invalidations.record()
        }
        turn.say()
        XCTAssertEqual(invalidations.count, 1)
    }

    func testCoalescedTextUpdatesOnlyTheReplyLeaf() {
        let model = SessionModel(client: Client(port: 0))
        let turn = Turn(prompt: "Review this project")
        turn.said("Starting the review.")
        turn.settle()
        guard case .say(let block) = turn.steps.first else {
            return XCTFail("Missing reply block")
        }
        let row = ChatTurn(turn: turn, number: 1, model: model)
        let reply = ChatReply(block: block)
        let rowChanges = Invalidations()
        let replyChanges = Invalidations()
        withObservationTracking { _ = row.body } onChange: { rowChanges.record() }
        withObservationTracking { _ = reply.body } onChange: { replyChanges.record() }

        turn.said("\n\nThe next paragraph.")
        XCTAssertEqual(replyChanges.count, 0, "Raw tokens must wait for the coalescer")
        turn.settle()
        XCTAssertEqual(replyChanges.count, 1, "The flushed text must appear in the reply")
        XCTAssertEqual(rowChanges.count, 0, "The rest of the turn must remain untouched")
    }

    func testScrollingAwayStaysUnpinnedWhenStreamingContinues() {
        // Content grew before the gesture began; its at-tail Boolean may already be false.
        var pinned = FollowsTail.following(true, atBottom: false, byHand: false)
        XCTAssertTrue(pinned)
        pinned = FollowsTail.following(pinned, atBottom: false, byHand: true)
        XCTAssertFalse(pinned)
        // Idle above the tail, followed by more content growth: neither can steal the offset.
        pinned = FollowsTail.following(pinned, atBottom: false, byHand: false)
        XCTAssertFalse(pinned)
        pinned = FollowsTail.following(pinned, atBottom: false, byHand: false)
        XCTAssertFalse(pinned)
    }

    func testGestureOwnsScrollingUntilItEndsEvenAtTheTail() {
        // Tracking starts at the bottom, before geometry has moved. Auto-follow must release
        // immediately; otherwise the size-change anchor competes with the first wheel events.
        var pinned = FollowsTail.following(true, atBottom: true, byHand: true)
        XCTAssertFalse(pinned)
        // A rubber band or fling reaches the tail while the gesture is still in flight.
        pinned = FollowsTail.following(pinned, atBottom: true, byHand: true)
        XCTAssertFalse(pinned)
        // The gesture settles at the tail: resume without needing another geometry transition.
        pinned = FollowsTail.following(pinned, atBottom: true, byHand: false)
        XCTAssertTrue(pinned)
    }

    /// Exercise the modifier's real phase/geometry callbacks as well as its decision function.
    /// Events go directly to this test's scroll view, never to the user's active Keel window.
    func testNativeScrollKeepsReadingPositionWhileContentGrows() async throws {
        let state = ScrollState()
        let host = NSHostingView(rootView: ScrollFixture(state: state))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 480, height: 320),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = host
        window.orderBack(nil)
        defer { window.orderOut(nil) }
        host.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(100))
        let scroll = try XCTUnwrap(scrollView(in: host))
        XCTAssertGreaterThan(state.offset, 1_000, "Fixture must open at its tail")

        try wheel(scroll, delta: 10, phase: .began)
        try await Task.sleep(for: .milliseconds(30))
        XCTAssertFalse(state.pinned, "The real phase callback must release auto-follow")
        try wheel(scroll, delta: 240, phase: .changed)
        try await Task.sleep(for: .milliseconds(30))
        try wheel(scroll, delta: 0, phase: .ended)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertFalse(state.pinned, "Finishing above the tail must stay unpinned")
        let readingOffset = state.offset
        XCTAssertLessThan(readingOffset, 2_050, "The wheel event must actually move the viewport")

        state.height += 1_000
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(state.offset, readingOffset, accuracy: 2,
                       "New output must not move a viewport that is reading earlier content")
    }

    private func scrollView(in view: NSView) -> NSScrollView? {
        if let scroll = view as? NSScrollView { return scroll }
        return view.subviews.lazy.compactMap { self.scrollView(in: $0) }.first
    }

    private func wheel(_ scroll: NSScrollView, delta: Int32, phase: CGScrollPhase) throws {
        let cg = try XCTUnwrap(CGEvent(scrollWheelEvent2Source: nil, units: .pixel,
                                      wheelCount: 1, wheel1: delta, wheel2: 0, wheel3: 0))
        cg.setIntegerValueField(.scrollWheelEventScrollPhase, value: Int64(phase.rawValue))
        scroll.scrollWheel(with: try XCTUnwrap(NSEvent(cgEvent: cg)))
    }

    @Observable
    fileprivate final class ScrollState {
        var pinned = true
        var height: CGFloat = 2_400
        var offset: CGFloat = 0
    }

    private struct ScrollFixture: View {
        @Bindable var state: ScrollState

        var body: some View {
            ScrollView {
                Color.gray.frame(height: state.height)
            }
            .defaultScrollAnchor(.bottom, for: .initialOffset)
            .defaultScrollAnchor(state.pinned ? .bottom : nil, for: .sizeChanges)
            .followsTail($state.pinned)
            .onScrollGeometryChange(for: CGFloat.self) { $0.contentOffset.y } action: { _, y in
                state.offset = y
            }
        }
    }

    private final class Invalidations: @unchecked Sendable {
        private let lock = NSLock()
        private var value = 0
        var count: Int { lock.withLock { value } }
        func record() { lock.withLock { value += 1 } }
    }
}
