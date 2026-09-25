import AppKit
import Observation
import SwiftUI
import XCTest
@testable import KeelApp

@MainActor
final class WorkspaceInteractionTests: XCTestCase {
    @Observable
    final class State {
        var stage = SessionWindow.Stage.turn
        var settings = false
        var terminal = false
        var command: String?
        var palette = false
        var starting = false
        var panel: SessionWindow.Panel? = .sessions
        var inspector = false
        var expanded = false
    }

    struct Fixture: View {
        let lanes: Lanes
        @Bindable var state: State

        var body: some View {
            Color.clear.frame(width: 100, height: 100)
                .modifier(WindowEvents(lanes: lanes, model: lanes.active,
                    stage: $state.stage, showSettings: $state.settings,
                    showTerminal: $state.terminal, terminalCommand: $state.command,
                    paletteOpen: $state.palette, starting: $state.starting,
                    panel: $state.panel, inspectorVisible: $state.inspector,
                    expandedInspector: $state.expanded))
        }
    }

    private func host(_ state: State, lanes: Lanes) -> NSHostingView<Fixture> {
        let host = NSHostingView(rootView: Fixture(lanes: lanes, state: state))
        host.frame = CGRect(x: 0, y: 0, width: 100, height: 100)
        host.layoutSubtreeIfNeeded()
        return host
    }

    func testKeyboardReviewCanReopenTheSameDestination() {
        let state = State()
        let lanes = Lanes(client: Client(port: 0), port: 0, remembers: false)
        let host = host(state, lanes: lanes)
        defer { withExtendedLifetime(host) {} }
        NotificationCenter.default.post(name: .keelShowStage, object: "Review")
        XCTAssertTrue(state.inspector)
        XCTAssertEqual(state.stage, .review)
        NotificationCenter.default.post(name: .keelToggleInspector, object: nil)
        XCTAssertFalse(state.inspector)
        NotificationCenter.default.post(name: .keelShowStage, object: "Review")
        XCTAssertTrue(state.inspector)
        XCTAssertEqual(state.stage, .review)
    }

    func testExpandRestoreAndReturnKeepTheDraftAndRequestComposerFocus() {
        let state = State()
        let lanes = Lanes(client: Client(port: 0), port: 0, remembers: false)
        lanes.active.prompt = "Keep this unfinished instruction"
        let before = lanes.active.focusComposerTick
        let host = host(state, lanes: lanes)
        defer { withExtendedLifetime(host) {} }
        NotificationCenter.default.post(name: .keelExpandInspector, object: nil)
        XCTAssertTrue(state.inspector)
        XCTAssertTrue(state.expanded)
        NotificationCenter.default.post(name: .keelExpandInspector, object: nil)
        XCTAssertTrue(state.inspector)
        XCTAssertFalse(state.expanded)
        NotificationCenter.default.post(name: .keelFocusComposer, object: nil)
        XCTAssertFalse(state.inspector)
        XCTAssertFalse(state.expanded)
        XCTAssertGreaterThan(lanes.active.focusComposerTick, before)
        XCTAssertEqual(lanes.active.prompt, "Keep this unfinished instruction")
    }

    func testSelectingTheSameTurnRequestsTraceAgainWithoutLosingSelection() {
        let model = SessionModel(client: Client(port: 0))
        let turn = UUID()
        model.focusedTurn = turn
        let first = model.workbench.traceRequest
        model.focusedTurn = turn
        XCTAssertEqual(model.focusedTurn, turn)
        XCTAssertGreaterThan(model.workbench.traceRequest, first)
    }

    func testNavigationAndTerminalCommandsRemainAvailable() {
        let state = State()
        let lanes = Lanes(client: Client(port: 0), port: 0, remembers: false)
        let host = host(state, lanes: lanes)
        defer { withExtendedLifetime(host) {} }
        NotificationCenter.default.post(name: .keelShowPanel, object: "files")
        XCTAssertEqual(state.panel, .files)
        NotificationCenter.default.post(name: .keelTogglePanel, object: nil)
        XCTAssertNil(state.panel)
        NotificationCenter.default.post(name: .keelTogglePanel, object: nil)
        XCTAssertEqual(state.panel, .files)
        NotificationCenter.default.post(name: .keelToggleTerminal, object: nil)
        XCTAssertTrue(state.terminal)
        NotificationCenter.default.post(name: .keelToggleInspector, object: nil)
        XCTAssertTrue(state.terminal, "Opening inspection must not destroy the terminal")
    }
}
