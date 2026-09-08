import Foundation
import SwiftUI

/// One window's world: its own daemon, its own project, its own features.
///
/// A window used to be a view onto the single daemon the app started, which is why two of them
/// were mirrors — same project, same tabs, same everything. A feature pulled out of the strip
/// is meant to be independent, so it gets a daemon of its own on its own port. Two projects
/// can then be open at once, and closing the window takes its daemon with it.
///
/// The conversation survives the move because Claude Code's transcripts are not the daemon's:
/// the new workspace resumes the same session id, in the same repository.
@MainActor
@Observable
final class Workspace {
    private static let ports = PortReservations(first: 7778, count: 40)

    let port: UInt16
    let client: Client
    let lanes: Lanes
    let pairing: PairingModel
    private let daemon: Daemon
    private let request: Detached?
    private(set) var failure: String?
    private(set) var ready = false
    private var stopped = false

    init(port: UInt16, request: Detached? = nil) {
        self.port = port
        self.request = request
        let c = Client(port: port)
        client = c
        pairing = PairingModel(client: c)
        // Not the window whose tabs are written down: the saved list is keyed by project alone, and
        // this window is `.restorationBehavior(.disabled)` — there is nothing here to come back to
        // across a launch, and restoring the other window's tabs would replace the lane this one
        // was torn out to show.
        lanes = Lanes(client: c, port: port, remembers: false, initialID: request?.lane ?? UUID())
        daemon = Daemon(port: port)
        request?.configure(lanes.active)
    }

    /// A port nothing is listening on. Probed rather than assumed: a second window on a machine
    /// that already had a stale daemon used to attach to it and mirror the first window again.
    static func reservePort() async -> UInt16? {
        await ports.reserve { await Daemon(port: $0).answering() }
    }

    static func releasePort(_ port: UInt16) async {
        await ports.release(port)
    }

    /// Start the daemon, open `project`, and resume `session` when there is one.
    func start(project: String, resume session: String?) async {
        guard !stopped, !Task.isCancelled else { return }
        do {
            try await daemon.start(resumeLast: false)
        } catch {
            guard !stopped, !Task.isCancelled else { return }
            failure = error.localizedDescription
            return
        }
        guard !stopped, !Task.isCancelled else { return }
        let first = lanes.active
        if !project.isEmpty {
            await first.open(project: project)
        }
        guard !stopped, !Task.isCancelled else { return }
        // Opening resets the first lane; restore its checkout before any pane reads the tree.
        request?.configure(first)
        await lanes.refreshShared()
        guard !stopped, !Task.isCancelled else { return }
        if let checkout = request?.worktree, first.worktree != checkout {
            failure = "The feature checkout “\(checkout)” is no longer available in this project."
            return
        }
        if let session, !session.isEmpty {
            await first.open(session: session, cwd: request?.resumeCwd)
        }
        guard !stopped, !Task.isCancelled else { return }
        if let request { first.title = request.title }
        ready = true
    }

    func reportFailure(_ message: String) {
        failure = message
    }

    func shutdown() {
        guard !stopped else { return }
        stopped = true
        ready = false
        lanes.shutdown()
        daemon.stop()
        let port = port
        Task { await Self.releasePort(port) }
    }
}

actor PortReservations {
    private let ports: Range<UInt16>
    private var reserved: Set<UInt16> = []

    init(first: UInt16, count: UInt16) {
        ports = first..<(first + count)
    }

    func reserve(occupied: @Sendable (UInt16) async -> Bool) async -> UInt16? {
        for port in ports where !reserved.contains(port) {
            reserved.insert(port)
            if !(await occupied(port)) { return port }
            reserved.remove(port)
        }
        return nil
    }

    func release(_ port: UInt16) {
        reserved.remove(port)
    }
}

/// What a torn-out window is told to open. Small and codable, because SwiftUI carries it as the
/// window's value: the lane it came from, the repository, and the conversation to resume.
struct Detached: Codable, Hashable, Identifiable {
    var lane: UUID
    var project: String
    var session: String?
    var title: String
    // Optional for window values encoded before these preferences travelled with the lane.
    var worktree: String? = nil
    var isolated: Bool? = nil
    var provider: SessionModel.Provider? = nil
    var mode: String? = nil
    var baseBranch: String? = nil
    var chosenName: String? = nil
    var sessionCwd: String? = nil
    var providerModel: String? = nil
    var id: UUID { lane }
}

extension Detached {
    /// A named checkout is the authority for both file requests and resumed conversations.
    /// A stale transcript cwd must not override it in the daemon's chat endpoint.
    var resumeCwd: String? {
        guard let worktree else { return sessionCwd }
        return URL(fileURLWithPath: project).appendingPathComponent(".keel/worktrees")
            .appendingPathComponent(worktree).path
    }

    @MainActor
    func configure(_ model: SessionModel) {
        model.sessionId = session
        model.title = title
        model.worktree = worktree
        model.isolated = isolated ?? (worktree != nil)
        model.provider = provider ?? .claude
        model.mode = mode ?? "acceptEdits"
        model.baseBranch = baseBranch
        model.chosenName = chosenName ?? ""
        model.sessionCwd = resumeCwd
        if let providerModel { model.claudeModel = providerModel }
    }

    @MainActor
    init(model: SessionModel) {
        self.init(lane: model.id, project: model.repoPath, session: model.sessionId,
                  title: model.title, worktree: model.worktree, isolated: model.isolated,
                  provider: model.provider, mode: model.mode, baseBranch: model.baseBranch,
                  chosenName: model.chosenName, sessionCwd: model.sessionCwd,
                  providerModel: model.claudeModel)
    }
}
