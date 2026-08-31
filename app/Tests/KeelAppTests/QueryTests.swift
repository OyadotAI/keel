import XCTest
@testable import KeelApp

/// The prompt travels as a GET query parameter (`api.rs` reads it with axum's `Query`, which is
/// `serde_urlencoded`). That is form-decoding, so a `+` on the wire arrives as a space. Foundation
/// will not encode `+` on its own — it is a legal query character — so `date +%H:%M:%S` reached
/// `claude` as `date  %H:%M:%S`, with no error and nothing in the transcript to suggest the prompt
/// had been altered. Found by reading back a monitored job's own command.
final class QueryTests: XCTestCase {

    private func encoded(_ query: [String: String]) -> String {
        var c = URLComponents(string: "http://127.0.0.1:7777/api/chat")!
        Client.encode(query, into: &c)
        return c.percentEncodedQuery ?? ""
    }

    /// What the server does with what we send: `+` is a space, `%2B` is a plus.
    private func formDecode(_ value: String) -> String {
        value
            .replacingOccurrences(of: "+", with: " ")
            .removingPercentEncoding ?? value
    }

    private func roundTrip(_ prompt: String) -> String {
        let q = encoded(["prompt": prompt])
        let value = String(q.dropFirst("prompt=".count))
        return formDecode(value)
    }

    func testAPlusSurvivesTheRoundTrip() {
        XCTAssertEqual(roundTrip("date +%H:%M:%S"), "date +%H:%M:%S")
    }

    func testTheShapesThatUsedToBeCorrupted() {
        for prompt in ["C++", #"grep -E 'a+b'"#, #"\d+"#, "1 + 1", "git log --format=%h+%s"] {
            XCTAssertEqual(roundTrip(prompt), prompt, "corrupted: \(prompt)")
        }
    }

    /// The encoding is a plus-for-`%2B` swap and nothing else — spaces, percent signs and unicode
    /// still have to come back intact, or the fix has traded one silent corruption for another.
    func testEverythingElseIsUnchanged() {
        for prompt in ["a b c", "100%", "a&b=c", "naïve café 日本語", "line\nbreak", "#hash"] {
            XCTAssertEqual(roundTrip(prompt), prompt, "corrupted: \(prompt)")
        }
    }

    func testAnEmptyQueryAddsNothing() {
        XCTAssertEqual(encoded([:]), "")
    }
}
