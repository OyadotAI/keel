import CryptoKit
import XCTest
@testable import KeelApp

final class PacketTests: XCTestCase {
    func testSignedPacketDetectsTampering() throws {
        let payload = PacketPayload(schema: 1, taskID: UUID(), title: "Fix auth",
                                    provider: "Claude Code", repository: "/repo",
                                    branch: "fix-auth", worktree: "fix-auth", head: "abc",
                                    files: ["src/auth.swift"], commands: [], gates: [],
                                    ready: true, blocker: nil, generatedAt: Date(timeIntervalSince1970: 1))
        let key = Curve25519.Signing.PrivateKey()
        let packet = try PacketSigner.sign(payload, using: key)
        XCTAssertTrue(PacketSigner.verify(packet))

        var changed = packet
        changed.payload.files.append("src/secret.swift")
        XCTAssertFalse(PacketSigner.verify(changed))
    }

    func testCanonicalPacketEncodingIsStable() throws {
        let payload = PacketPayload(schema: 1, taskID: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!,
                                    title: "One", provider: "Codex", repository: "/repo",
                                    branch: nil, worktree: nil, head: nil, files: [], commands: [],
                                    gates: [], ready: false, blocker: "not ready",
                                    generatedAt: Date(timeIntervalSince1970: 1))
        XCTAssertEqual(try PacketSigner.canonical(payload), try PacketSigner.canonical(payload))
    }
}
