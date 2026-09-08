import AppKit
import SwiftUI

/// One setup flow shared by first launch and Settings. It never sends an agent prompt.
@MainActor
@Observable
final class ClaudeSetup {
    struct Status: Decodable, Equatable {
        var installed: Bool
        var version: String?
        var authenticated: Bool
        var account: String?
        var plan: String?
        var brew: Bool
    }
    enum Action { case install, login }
    let client: Client
    var status: Status?
    private(set) var action: Action?
    private(set) var output = ""
    private(set) var failure: String?
    @ObservationIgnored private var task: Task<Void, Never>?
    private var revision = 0

    init(client: Client, status: Status? = nil) {
        self.client = client
        self.status = status
    }

    var ready: Bool { status?.installed == true && status?.authenticated == true }
    var busy: Bool { action != nil }
    var links: [URL] { Self.loginLinks(in: output) }

    static func loginLinks(in output: String) -> [URL] {
        guard let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue) else { return [] }
        var seen = Set<URL>()
        return detector.matches(in: output, range: NSRange(output.startIndex..., in: output))
            .compactMap(\.url)
            .filter { $0.scheme == "https" && $0.user == nil && $0.password == nil && seen.insert($0).inserted }
    }

    func refresh() async {
        revision += 1
        let mine = revision
        do {
            let fresh: Status = try await client.get("/api/claude")
            guard !Task.isCancelled, revision == mine else { return }
            status = fresh
            failure = nil
        } catch {
            guard !Task.isCancelled, revision == mine else { return }
            failure = "Could not check Claude Code: \(error.localizedDescription)"
        }
    }

    func start(_ next: Action) {
        guard !busy else { return }
        action = next
        failure = nil
        output = ""
        task = Task {
            defer { action = nil; task = nil }
            var exitCode: Int?
            do {
                let endpoint = next == .install ? "/api/claude/install" : "/api/claude/login"
                for try await event in client.events(endpoint) {
                    guard !Task.isCancelled else { return }
                    switch event.name {
                    case "line", "fatal":
                        output = String((output + event.data + "\n").suffix(64_000))
                        if event.name == "fatal" { failure = event.data }
                    case "done": exitCode = Int(event.data)
                    default: break
                    }
                }
                guard !Task.isCancelled else { return }
                if exitCode == 0 {
                    await refresh()
                    guard !Task.isCancelled else { return }
                    if next == .install && status?.installed != true {
                        failure = "The installer finished, but Claude Code is not available yet. Check the output and try Check again."
                    } else if next == .login && !ready {
                        failure = "Sign-in finished, but Claude Code has not confirmed the account. Complete the browser step, then check again."
                    }
                } else if failure == nil {
                    failure = exitCode.map { "Setup exited with code \($0). Check the output, then retry." }
                        ?? "Setup disconnected before finishing. Check again before retrying."
                }
            } catch {
                guard !Task.isCancelled else { return }
                failure = error.localizedDescription
            }
        }
    }

    func cancel() {
        revision += 1
        task?.cancel()
        // The task owns clearing `action`, so a replacement cannot race its cleanup.
    }
}

struct ClaudeSetupCard: View {
    @Bindable var setup: ClaudeSetup
    var showsStatus = true

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            if showsStatus {
                HStack(spacing: K.S.sm) {
                    Image(systemName: "terminal").font(K.F.title).foregroundStyle(K.C.accent)
                    VStack(alignment: .leading, spacing: K.S.xxs) {
                        Text("Claude Code").font(K.F.body.weight(.semibold)).foregroundStyle(K.C.text)
                        Text(setup.ready ? (setup.status?.account ?? "Connected") : "Your Claude account. Set up right here.")
                            .font(K.F.small).foregroundStyle(K.C.dim)
                    }
                    Spacer()
                    if setup.ready {
                        Label("Ready", systemImage: "checkmark.circle.fill")
                            .font(K.F.small).foregroundStyle(K.C.add)
                    }
                }
            }

            if setup.status == nil {
                Text("Checking Claude Code…").font(K.F.small).foregroundStyle(K.C.dim)
            } else if !setup.ready {
                HStack(spacing: K.S.lg) {
                    step("1", "Install", done: setup.status?.installed == true)
                    Image(systemName: "chevron.right").font(K.F.tiny).foregroundStyle(K.C.faint)
                    step("2", "Sign in", done: setup.status?.authenticated == true)
                    Image(systemName: "chevron.right").font(K.F.tiny).foregroundStyle(K.C.faint)
                    step("3", "Build", done: false)
                }
                if setup.status?.installed != true {
                    Text("Install Anthropic's native CLI. No Node.js or Homebrew required.")
                        .font(K.F.small).foregroundStyle(K.C.dim)
                    Text("curl -fsSL https://claude.ai/install.sh | bash")
                        .font(K.F.codeSmall).foregroundStyle(K.C.dim).textSelection(.enabled)
                    Button(setup.busy ? "Installing Claude Code…" : "Install Claude Code") { setup.start(.install) }
                        .buttonStyle(FilledButton()).disabled(setup.busy)
                } else {
                    Text("Sign in with your Claude account in the browser. Claude Code manages the credentials; Keel does not ask for your password.")
                        .font(K.F.small).foregroundStyle(K.C.dim)
                    Button(setup.busy ? "Waiting for browser sign-in…" : "Sign in with Claude") { setup.start(.login) }
                        .buttonStyle(FilledButton()).disabled(setup.busy)
                }
            }

            if setup.action == .login {
                ForEach(setup.links, id: \.self) { url in
                    Link(destination: url) {
                        Label("Open sign-in page · \(url.host ?? "browser")", systemImage: "arrow.up.right.square")
                            .font(K.F.small)
                    }
                }
            }
            if let failure = setup.failure {
                Text(failure).font(K.F.small).foregroundStyle(K.C.del).textSelection(.enabled)
            }
            if !setup.output.isEmpty {
                ScrollView {
                    Text(setup.output).font(K.F.codeSmall).foregroundStyle(K.C.dim)
                        .frame(maxWidth: .infinity, alignment: .leading).textSelection(.enabled)
                }
                .frame(maxHeight: 130).padding(K.S.sm)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            }
            HStack(spacing: K.S.md) {
                Button("Check again") { Task { await setup.refresh() } }
                    .buttonStyle(QuietButton()).disabled(setup.busy)
                if setup.busy {
                    Button("Cancel setup") { setup.cancel() }.buttonStyle(QuietButton())
                }
                Spacer()
                Link("Setup help ↗", destination: URL(string: "https://code.claude.com/docs/en/setup")!)
                    .font(K.F.small)
            }
        }
        .padding(K.S.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
    }

    private func step(_ number: String, _ title: String, done: Bool) -> some View {
        HStack(spacing: K.S.xs) {
            if done { Image(systemName: "checkmark.circle.fill").foregroundStyle(K.C.add) }
            else { Text(number).font(K.F.codeSmall).foregroundStyle(K.C.dim) }
            Text(title).foregroundStyle(done ? K.C.text : K.C.dim)
        }
        .font(K.F.small.weight(.medium))
    }
}
