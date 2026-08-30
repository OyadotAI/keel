import CryptoKit
import Foundation
import Security

struct PacketPayload: Codable {
    struct Command: Codable {
        var tool: String
        var command: String
        var reason: String?
        var failed: Bool
    }

    struct Gate: Codable {
        var status: String
        var command: String?
    }

    var schema = 1
    var taskID: UUID
    var title: String
    var provider: String
    var repository: String
    var branch: String?
    var worktree: String?
    var head: String?
    var files: [String]
    var commands: [Command]
    var gates: [Gate]
    var ready: Bool
    var blocker: String?
    var generatedAt: Date

    init(schema: Int, taskID: UUID, title: String, provider: String, repository: String,
         branch: String?, worktree: String?, head: String?, files: [String], commands: [Command],
         gates: [Gate], ready: Bool, blocker: String?, generatedAt: Date) {
        self.schema = schema
        self.taskID = taskID
        self.title = title
        self.provider = provider
        self.repository = repository
        self.branch = branch
        self.worktree = worktree
        self.head = head
        self.files = files
        self.commands = commands
        self.gates = gates
        self.ready = ready
        self.blocker = blocker
        self.generatedAt = generatedAt
    }

    @MainActor
    init(model: SessionModel) {
        let packet = ReviewPacket(model: model)
        taskID = packet.taskID
        title = packet.title
        provider = packet.provider.rawValue
        repository = model.repoPath
        branch = packet.branch
        worktree = packet.worktree
        head = model.commits.first?.sha
        files = packet.files.sorted()
        commands = packet.commands.map {
            Command(tool: $0.tool, command: $0.subject, reason: $0.reason, failed: $0.failed)
        }
        gates = packet.gates.map {
            switch $0 {
            case .notRun: Gate(status: "not_run", command: nil)
            case .running(let command): Gate(status: "running", command: command)
            case .passed(let command, _): Gate(status: "passed", command: command)
            case .failed(let command, _): Gate(status: "failed", command: command)
            case .none(let reason): Gate(status: "unverified", command: reason)
            }
        }
        ready = packet.blocker == nil
        blocker = packet.blocker
        generatedAt = Date()
    }
}

struct SignedPacket: Codable {
    var payload: PacketPayload
    var publicKey: String
    var signature: String
}

enum PacketSigner {
    static func canonical(_ payload: PacketPayload) throws -> Data {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(payload)
    }

    static func sign(_ payload: PacketPayload, using key: Curve25519.Signing.PrivateKey) throws -> SignedPacket {
        let signature = try key.signature(for: canonical(payload))
        return SignedPacket(payload: payload,
                            publicKey: key.publicKey.rawRepresentation.base64EncodedString(),
                            signature: signature.base64EncodedString())
    }

    static func verify(_ packet: SignedPacket) -> Bool {
        guard let publicData = Data(base64Encoded: packet.publicKey),
              let signature = Data(base64Encoded: packet.signature),
              let key = try? Curve25519.Signing.PublicKey(rawRepresentation: publicData),
              let payload = try? canonical(packet.payload) else { return false }
        return key.isValidSignature(signature, for: payload)
    }
}

enum PacketStore {
    private static let service = "ai.oya.keel.review-packet"
    private static let account = "device-signing-key-v1"

    @MainActor
    static func save(model: SessionModel) throws -> URL {
        let packet = try make(model: model)
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        let data = try encoder.encode(packet)
        let root = try FileManager.default.url(for: .applicationSupportDirectory,
                                               in: .userDomainMask,
                                               appropriateFor: nil,
                                               create: true)
            .appending(path: "Keel/packets", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let url = root.appending(path: model.id.uuidString.lowercased() + ".json")
        try data.write(to: url, options: .atomic)
        return url
    }

    @MainActor
    static func markdown(model: SessionModel) throws -> String {
        let packet = try make(model: model)
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        let json = String(decoding: try encoder.encode(packet), as: UTF8.self)
        let payload = packet.payload
        let status = payload.ready ? "Ready for human merge decision" : "Blocked: \(payload.blocker ?? "unknown")"
        let files = payload.files.map { "- `\($0)`" }.joined(separator: "\n")
        let gates = payload.gates.map { "- \($0.status): `\($0.command ?? "—")`" }.joined(separator: "\n")
        return """
        <!-- keel-review-packet:v1 -->
        ## Keel review packet

        **\(status)** · \(payload.provider) · task `\(payload.taskID.uuidString.lowercased())`

        **Files touched (\(payload.files.count))**
        \(files.isEmpty ? "- None" : files)

        **Quality evidence**
        \(gates.isEmpty ? "- No gate recorded" : gates)

        <details><summary>Verify signed packet</summary>

        ```json
        \(json)
        ```
        </details>
        """
    }

    @MainActor
    private static func make(model: SessionModel) throws -> SignedPacket {
        try PacketSigner.sign(PacketPayload(model: model), using: signingKey())
    }

    private static func signingKey() throws -> Curve25519.Signing.PrivateKey {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecSuccess, let data = item as? Data {
            return try Curve25519.Signing.PrivateKey(rawRepresentation: data)
        }
        guard status == errSecItemNotFound else {
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(status))
        }
        let key = Curve25519.Signing.PrivateKey()
        let add: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData as String: key.rawRepresentation,
        ]
        let added = SecItemAdd(add as CFDictionary, nil)
        guard added == errSecSuccess else {
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(added))
        }
        return key
    }
}
