import Foundation

/// The conversation and what is happening to it: the turns, whether one is running, and where a
/// transcript read stands.
///
/// One per lane, read through the lane's forwarding accessors so no view changed with the move.
/// The consumer that writes it — `apply`, `close`, `record` — stays on the lane for now, because
/// it also reaches the Designer, the approvals and git; what moved is the state, which used to
/// sit among a hundred other things on the same class. What is here is what a test of the turn
/// lifecycle sets up and asserts on, and nothing else.
@MainActor
@Observable
final class SessionStore {
    var sessionId: String?
    var turns: [Turn] = []
    var running = false
    /// The turn's stream has ended but the tree is not finished with.
    var settling = false
    /// What the daemon is doing before the agent exists, said in the working bar.
    var preparing: String?
    /// The last event of any kind, heartbeat included. Sixty seconds without one is a dead stream.
    var lastEventAt = Date()
    /// The last event from the agent itself. Five minutes without one is a turn that has stalled.
    var lastProgressAt = Date()
    var stallReported = false
    /// Where the transcript read stands: reading, failed with a reason, or neither.
    var replay: SessionModel.Replay = .none
    /// The transcript is being followed: the session is running somewhere Keel does not own it.
    var following = false
    var replayDropped = 0
    var replayDroppedBytes = 0
    /// Whether the turn on screen is one this lane started.
    var owned = false
    /// Turns read from the transcript, held back until the daemon says it has caught up.
    var catchUp: [Turn] = []
    /// The turn a followed stream is writing into.
    var followed: Turn?
}
