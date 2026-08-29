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
        cfg.timeoutIntervalForRequest = 3600
        cfg.timeoutIntervalForResource = 86_400
        session = URLSession(configuration: cfg)
    }

    struct Failure: Error, LocalizedError {
        let status: Int
        let body: String
        var errorDescription: String? { body.isEmpty ? "HTTP \(status)" : body }
    }

    private func url(_ path: String, _ query: [String: String] = [:]) -> URL {
        var c = URLComponents(url: base.appendingPathComponent(path), resolvingAgainstBaseURL: false)!
        if !query.isEmpty {
            c.queryItems = query.map { URLQueryItem(name: $0.key, value: $0.value) }
        }
        return c.url!
    }

    /// Errors come back as a bare text body, not JSON — see `ApiResult` in connect.rs. A client
    /// that assumes JSON on failure reports "decoding error" for every real message the server
    /// took the trouble to write.
    private func check(_ data: Data, _ response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) else { return }
        throw Failure(status: http.statusCode, body: String(decoding: data, as: UTF8.self))
    }

    func get<T: Decodable>(_ path: String, _ query: [String: String] = [:], as: T.Type = T.self) async throws -> T {
        let (data, response) = try await session.data(from: url(path, query))
        try check(data, response)
        return try JSONDecoder().decode(T.self, from: data)
    }

    /// Bytes, for a file the viewer shows.
    func raw(_ path: String, _ query: [String: String] = [:]) async throws -> Data {
        let (data, response) = try await session.data(from: url(path, query))
        try check(data, response)
        return data
    }

    @discardableResult
    func post<T: Decodable>(_ path: String, body: some Encodable, _ query: [String: String] = [:],
                            as: T.Type = T.self) async throws -> T {
        var req = URLRequest(url: url(path, query))
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

    /// Stream an SSE endpoint.
    ///
    /// Written against `URLSession.bytes` rather than a library: the format is two field names and
    /// a blank line, and Keel only ever emits single-line `data:` payloads.
    nonisolated func events(_ path: String, _ query: [String: String] = [:]) -> AsyncThrowingStream<Event, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var c = URLComponents(url: base.appendingPathComponent(path), resolvingAgainstBaseURL: false)!
                    if !query.isEmpty { c.queryItems = query.map { URLQueryItem(name: $0.key, value: $0.value) } }
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

    mutating func feed(_ line: String) -> Client.Event? {
        if line.isEmpty { name = "message"; return nil }
        if let v = line.dropPrefixIfPresent("event:") {
            name = v.trimmingCharacters(in: .whitespaces)
            return nil
        }
        if let v = line.dropPrefixIfPresent("data:") {
            // Exactly one space of padding, which is what the format says and what matters here:
            // `/api/verify` streams raw compiler output as `line` events, and that output is
            // indented. Dropping every leading space would silently reflow every error message
            // Keel shows.
            return Client.Event(name: name, data: String(v.hasPrefix(" ") ? v.dropFirst() : v))
        }
        // A comment line (`:keep-alive`) or anything unrecognised is not an event.
        return nil
    }
}

extension String {
    func dropPrefixIfPresent(_ p: String) -> Substring? {
        hasPrefix(p) ? dropFirst(p.count) : nil
    }
}
