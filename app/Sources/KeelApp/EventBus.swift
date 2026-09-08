import Foundation

extension Wire {
    /// One frame on `/api/events`: what changed, which checkout it is about, and the payload
    /// for the kinds that carry one.
    struct ProjectEvent: Sendable {
        var kind: String
        var wt: String?
        var seq: Int
        var json: String

        /// The frame's `data`, for the kinds that carry one.
        func payload<T: Decodable>(_ type: T.Type = T.self) -> T? {
            guard let data = json.data(using: .utf8) else { return nil }
            return (try? JSONDecoder().decode(Payload<T>.self, from: data))?.data
        }
    }
}

private struct Payload<D: Decodable>: Decodable { var data: D }

/// The daemon's project events, as one subscription per window fanned out to whoever asks.
///
/// This is what replaces the polls: the session list every three seconds, the jobs every two or
/// fifteen per lane, the dev server forty times after a start. Each store subscribes once; the
/// daemon says what changed and the store reads that thing again. A connection that drops comes
/// back on its own, and says so: `connected` is drawn in the status bar, and every subscriber is
/// handed a `connected` frame on each reconnect because whatever happened in the gap is gone.
@MainActor
@Observable
final class DaemonEvents {
    private(set) var connected = false
    private(set) var failure: Fault?
    @ObservationIgnored private var subscribers: [UUID: AsyncStream<Wire.ProjectEvent>.Continuation] = [:]
    @ObservationIgnored private var task: Task<Void, Never>?

    deinit {
        task?.cancel()
        for continuation in subscribers.values { continuation.finish() }
    }

    func subscribe() -> AsyncStream<Wire.ProjectEvent> {
        let id = UUID()
        return AsyncStream { continuation in
            subscribers[id] = continuation
            continuation.onTermination = { [weak self] _ in
                Task { @MainActor in self?.subscribers.removeValue(forKey: id) }
            }
        }
    }

    /// Connect, and keep connecting. Backs off from half a second to eight.
    func start(_ client: Client) {
        guard task == nil else { return }
        task = Task { [weak self] in
            var backoff: Double = 0.5
            while !Task.isCancelled {
                do {
                    for try await event in client.events("/api/events") {
                        guard let self, !Task.isCancelled else { return }
                        if event.name == Client.keepAlive { continue }
                        if !self.connected {
                            self.connected = true
                            self.failure = nil
                            backoff = 0.5
                        }
                        self.deliver(Self.frame(event))
                    }
                } catch {
                    if !Task.isCancelled {
                        self?.failure = Fault(what: "the daemon's events", why: error.localizedDescription)
                    }
                }
                guard !Task.isCancelled, self != nil else { return }
                if self?.connected == true {
                    self?.connected = false
                    self?.deliver(Wire.ProjectEvent(kind: "disconnected", wt: nil, seq: 0, json: "{}"))
                }
                try? await Task.sleep(for: .seconds(backoff))
                backoff = min(backoff * 2, 8)
            }
        }
    }

    func stop() {
        task?.cancel()
        task = nil
        connected = false
        failure = nil
        for continuation in subscribers.values { continuation.finish() }
        subscribers.removeAll()
    }

    /// A test's way in: the same door the stream uses.
    func inject(_ event: Wire.ProjectEvent) { deliver(event) }

    private static func frame(_ event: Client.Event) -> Wire.ProjectEvent {
        struct Head: Decodable { var seq: Int?; var wt: String? }
        let head = event.data.data(using: .utf8).flatMap { try? JSONDecoder().decode(Head.self, from: $0) }
        return Wire.ProjectEvent(kind: event.name, wt: head?.wt, seq: head?.seq ?? 0, json: event.data)
    }

    private func deliver(_ event: Wire.ProjectEvent) {
        for continuation in subscribers.values { continuation.yield(event) }
    }
}
