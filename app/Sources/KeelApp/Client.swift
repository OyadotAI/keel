import Foundation

/// Talking to the `keel serve` daemon on loopback.
///
/// One instance per app, shared by every window: the daemon holds one open project, so a second
/// client would be a second opinion about the same thing.
actor Client {
    let base: URL
    private let session: URLSession

    init(port: UInt16) {
        base = URL(string: "http://127.0.0.1:\(port)")!
        let cfg = URLSessionConfiguration.ephemeral
        // The chat stream is open for as long as a turn runs, which is minutes. The default
        // request timeout would cut a long turn off mid-thought and look like a crash.
        //
        // It is the *session's* ceiling, and every ordinary call used to inherit it: a daemon
        // that never answered `/api/open` left the window dimmed on "Opening …" for an hour with
        // nothing to click and nothing to cancel. The stream keeps the hour; everything else gets
        // `Self.ordinary` per request, so a call that will not come back fails and says so.
        cfg.timeoutIntervalForRequest = 3600
        cfg.timeoutIntervalForResource = 86_400
        // Every stream holds a connection for the life of a lane — the events stream, one tail
        // per lane, a chat stream per turn — and the default ceiling is six per host. The seventh
        // request queued behind them for ever: a lane opened from History sat on "Opening this
        // session…" with the daemon having already answered. Loopback connections are cheap;
        // a window that cannot ask is not.
        cfg.httpMaximumConnectionsPerHost = Self.connections
        session = URLSession(configuration: cfg)
    }

    /// Connections the session may hold to the daemon at once. Well past what a window with a
    /// dozen lanes needs, so the limit is never the thing a person is waiting on.
    static let connections = 64

    /// What the session was actually configured with, for the test that pins it.
    func maximumConnections() -> Int { session.configuration.httpMaximumConnectionsPerHost }

    struct Failure: Error, LocalizedError {
        let status: Int
        let body: String
        var errorDescription: String? { body.isEmpty ? "HTTP \(status)" : body }
    }

    /// How long a request that is not the chat stream may take.
    ///
    /// Generous, because the daemon scans repositories and a cold `/api/open` on a large one is
    /// genuinely slow — but finite, because the alternative is a window that never comes back.
    static let ordinary: TimeInterval = 90

    private func request(_ path: String, _ query: [String: String] = [:]) -> URLRequest {
        var req = URLRequest(url: url(path, query))
        req.timeoutInterval = Self.ordinary
        return req
    }

    /// `URLQueryItem` percent-encodes `%` and space but leaves `+` alone, because `+` is a legal
    /// character in a query string. The daemon reads the query with axum's `Query`, which is
    /// `serde_urlencoded` — form semantics, where `+` *means* space. So a prompt of
    /// `date +%H` went on the wire as `date%20+%25H` and reached `claude` as `date  %H`: two
    /// spaces, no error, no sign anything had happened. Every query is built through here so the
    /// fix covers `C++`, `\d+` and the rest, not just the prompt that exposed it.
    static func encode(_ query: [String: String], into c: inout URLComponents) {
        guard !query.isEmpty else { return }
        c.queryItems = query.map { URLQueryItem(name: $0.key, value: $0.value) }
        // Safe as a blind replacement: URLComponents writes a space as `%20`, never as `+`, so
        // every `+` left in the encoded query is a literal one.
        c.percentEncodedQuery = c.percentEncodedQuery?.replacingOccurrences(of: "+", with: "%2B")
    }

    private func url(_ path: String, _ query: [String: String] = [:]) -> URL {
        var c = URLComponents(url: base.appendingPathComponent(path), resolvingAgainstBaseURL: false)!
        Self.encode(query, into: &c)
        return c.url!
    }

    /// Errors come back as a bare text body, not JSON — see `ApiResult` in connect.rs. A client
    /// that assumes JSON on failure reports "decoding error" for every real message the server
    /// took the trouble to write.
    private func check(_ data: Data, _ response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) else { return }
        throw Failure(status: http.statusCode, body: String(decoding: data, as: UTF8.self))
    }

    /// A read for a store: what could not be read, and why, as a value rather than a throw.
    ///
    /// `try?` on a store read was the quiet half of every "blank pane" report: the panel kept
    /// whatever it last had with nothing on screen saying the daemon had stopped answering. A
    /// `Result` cannot be swallowed by accident — the caller has to write the failure case, and
    /// writing it is drawing it.
    func fetch<T: Decodable & Sendable>(_ what: String, _ path: String,
                                        _ query: [String: String] = [:]) async -> Result<T, Fault> {
        do { return .success(try await get(path, query)) } catch {
            return .failure(Fault(what: what, why: error.localizedDescription))
        }
    }

    func get<T: Decodable>(_ path: String, _ query: [String: String] = [:], as: T.Type = T.self) async throws -> T {
        let (data, response) = try await session.data(for: request(path, query))
        try check(data, response)
        return try JSONDecoder().decode(T.self, from: data)
    }

    /// Bytes, for a file the viewer shows.
    func raw(_ path: String, _ query: [String: String] = [:]) async throws -> Data {
        let (data, response) = try await session.data(for: request(path, query))
        try check(data, response)
        return data
    }

    @discardableResult
    func post<T: Decodable>(_ path: String, body: some Encodable, _ query: [String: String] = [:],
                            as: T.Type = T.self) async throws -> T {
        var req = request(path, query)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "content-type")
        req.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await session.data(for: req)
        try check(data, response)
        return try JSONDecoder().decode(T.self, from: data)
    }

    /// One server-sent event.
    struct Event: Sendable {
        var name: String
        var data: String
    }

    /// The server's heartbeat, surfaced as an event so a silent stream can be told from a dead
    /// one. Never sent by a handler, so it cannot collide with a real event name.
    static let keepAlive = "\u{0000}keep-alive"

    /// Stream an SSE endpoint.
    ///
    /// Written against `URLSession.bytes` rather than a library: the format is two field names and
    /// a blank line, and Keel only ever emits single-line `data:` payloads.
    nonisolated func events(_ path: String, _ query: [String: String] = [:]) -> AsyncThrowingStream<Event, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var c = URLComponents(url: base.appendingPathComponent(path), resolvingAgainstBaseURL: false)!
                    Client.encode(query, into: &c)
                    var req = URLRequest(url: c.url!)
                    req.setValue("text/event-stream", forHTTPHeaderField: "accept")
                    let (bytes, response) = try await session.bytes(for: req)
                    if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                        throw Failure(status: http.statusCode, body: "")
                    }
                    var parser = SSEParser()
                    for try await line in bytes.lines {
                        if let event = parser.feed(line) { continuation.yield(event) }
                    }
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

/// The server-sent-events line format, on its own so it can be tested without a socket.
///
/// Deliberately small: Keel emits one `event:` line then one `data:` line per message, and the
/// event name resets at the blank line between them. Anything else in the spec — `id:`, `retry:`,
/// multi-line data — Keel never sends, so parsing it would be code with no caller.
struct SSEParser {
    private var name = "message"
    /// Whether a `data:` line has been seen since the last `event:`.
    private var carried = false

    mutating func feed(_ line: String) -> Client.Event? {
        // The end of a frame. An `event:` with no `data:` is still an event, and dropping it is
        // how a whole message disappears between two layers that each look correct.
        //
        // Measured: axum writes **no `data:` line at all** when the payload is empty, so
        // `/api/session/tail`'s `caught-up` — the signal that a replayed conversation is ready to
        // draw — reached the socket as `event: caught-up` followed by a blank line, and was
        // swallowed here. The pane sat on "Opening this session…" with the whole transcript
        // already decoded behind it.
        if line.isEmpty {
            defer { name = "message"; carried = false }
            guard !carried, name != "message" else { return nil }
            return Client.Event(name: name, data: "")
        }
        if let v = line.dropPrefixIfPresent("event:") {
            name = v.trimmingCharacters(in: .whitespaces)
            carried = false
            return nil
        }
        if let v = line.dropPrefixIfPresent("data:") {
            // Exactly one space of padding, which is what the format says and what matters here:
            // `/api/verify` streams raw compiler output as `line` events, and that output is
            // indented. Dropping every leading space would silently reflow every error message
            // Keel shows.
            carried = true
            return Client.Event(name: name, data: String(v.hasPrefix(" ") ? v.dropFirst() : v))
        }
        // A comment line is the server's heartbeat. It carries nothing, but the fact that it
        // arrived is the one thing a reader needs to tell "the agent is thinking" from "the
        // daemon is gone" — and those look identical when the stream's own timeout is an hour.
        // Named so a consumer that does not care can ignore it in one line.
        if line.hasPrefix(":") { return Client.Event(name: Client.keepAlive, data: "") }
        return nil
    }
}

extension String {
    func dropPrefixIfPresent(_ p: String) -> Substring? {
        hasPrefix(p) ? dropFirst(p.count) : nil
    }
}

/// A read a store could not make: which one, and what the transport said.
///
/// Not `Client.Failure`, which is one HTTP response; this is the shape a pane draws, and it
/// carries no `Fix` because a store cannot know one — a pane that does (`NotARepo` offering
/// `git init`) still renders it. Stale data beside a fault beats a blank pane.
struct Fault: Equatable, Error, Sendable {
    /// What was being read, in the words a person would use: "git status", "the file tree".
    let what: String
    let why: String
}
