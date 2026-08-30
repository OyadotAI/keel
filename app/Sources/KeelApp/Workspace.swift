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
    let port: UInt16
    let client: Client
    let lanes: Lanes
    let pairing: PairingModel
    private let daemon: Daemon
    private(set) var failure: String?
    private(set) var ready = false

    init(port: UInt16) {
        self.port = port
        let c = Client(port: port)
        client = c
        pairing = PairingModel(client: c)
        lanes = Lanes(client: c, port: port)
        daemon = Daemon(port: port)
    }

    /// A port nothing is listening on. Probed rather than assumed: a second window on a machine
    /// that already had a stale daemon used to attach to it and mirror the first window again.
    static func freePort(after first: UInt16 = 7778) async -> UInt16 {
        for port in first..<(first + 40) where !(await Daemon(port: port).answering()) {
            return port
        }
        return first
    }

    /// Start the daemon, open `project`, and resume `session` when there is one.
    func start(project: String, resume session: String?) async {
        do {
            try await daemon.start(resumeLast: false)
        } catch {
            failure = error.localizedDescription
            return
        }
        ready = true
        let first = lanes.active
        if !project.isEmpty {
            try? await first.openProject(project)
        }
        await lanes.refreshShared()
        if let session, !session.isEmpty {
            await first.open(session: session)
        }
    }

    func shutdown() {
        for lane in lanes.lanes { lane.stop() }
        daemon.stop()
    }
}

/// What a torn-out window is told to open. Small and codable, because SwiftUI carries it as the
/// window's value: the lane it came from, the repository, and the conversation to resume.
struct Detached: Codable, Hashable, Identifiable {
    var lane: UUID
    var project: String
    var session: String?
    var title: String
    var id: UUID { lane }
}
