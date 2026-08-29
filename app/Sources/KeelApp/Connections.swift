import SwiftUI

/// The CLIs Keel drives, and how to get them working.
///
/// Keel never asks for a long-lived token. Every one of these tools has its own login — a browser
/// handshake, a device code, a password prompt — and Keel hosts or streams it rather than
/// reimplementing it. The credential goes to the CLI, and Keel reads the CLI's answer to "who am
/// I" rather than storing anything of its own.
struct ConnectionsSettings: View {
    let client: Client

    @State private var tools: [Tool] = []
    @State private var busy: String?
    @State private var log = ""
    @State private var stored: Stored?

    struct Stored: Decodable {
        var github: JSONValue?
        var cloudflare: JSONValue?
    }

    struct Tool: Decodable, Identifiable, Sendable {
        var id: String
        var label: String
        var installed: Bool
        var version: String?
        var authenticated: Bool
        var identity: String?
        /// Why it is not connected, in its own words.
        var blocked: String?
        /// A command to hand a terminal when Keel cannot drive the flow.
        var setup: String?
    }

    private var broken: [Tool] { tools.filter { !$0.installed || !$0.authenticated } }

    var body: some View {
        Form {
            // What is wrong, first. A list of nine rows where two matter is a list you read nine
            // times to find the two.
            Section {
                if tools.isEmpty {
                    // Seconds, because each tool is asked "who am I" in turn. Motion says it is
                    // working; a grey word says it is stuck.
                    HStack(spacing: K.S.sm) {
                        Sweep()
                        Text("Checking which tools are installed and signed in…")
                            .font(K.F.small).foregroundStyle(K.C.dim)
                    }
                } else if broken.isEmpty {
                    HStack(spacing: K.S.sm) {
                        Image(systemName: "checkmark.circle.fill")
                            .font(.system(size: 11)).foregroundStyle(K.C.add)
                        Text("All \(tools.count) tools installed and signed in.")
                            .font(K.F.small).foregroundStyle(K.C.dim)
                    }
                } else {
                    HStack(alignment: .top, spacing: K.S.sm) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(.system(size: 11)).foregroundStyle(K.C.warn)
                        VStack(alignment: .leading, spacing: K.S.xxs) {
                            Text(broken.map(\.label).joined(separator: ", "))
                                .font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                                .fixedSize(horizontal: false, vertical: true)
                            Text(broken.count == 1
                                 ? "is not ready. Nothing that needs it will work."
                                 : "are not ready. Nothing that needs them will work.")
                                .font(K.F.micro).foregroundStyle(K.C.dim)
                        }
                    }
                }
            }

            // A grid of cards, not a section per tool: nine sections was a page of scrolling
            // to answer "is docker signed in".
            Section {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 230), spacing: K.S.sm)],
                          alignment: .leading, spacing: K.S.sm) {
                    ForEach(tools) { t in
                        ToolCard(tool: t, busy: busy == t.id, anyBusy: busy != nil,
                                 install: { stream("/api/cli/install", ["id": t.id], t.id) },
                                 login: { stream("/api/cli/login", ["id": t.id], t.id) })
                    }
                }
            }

            Section {
                TokenRow(client: client, label: "GitHub", stored: stored?.github != nil,
                         path: "/api/connect/github",
                         help: "A personal access token, stored in the login keychain. Only needed "
                             + "for what `gh` cannot do for you.")
                TokenRow(client: client, label: "Cloudflare", stored: stored?.cloudflare != nil,
                         path: "/api/connect/cloudflare",
                         help: "A scoped, rotatable API token. Cloudflare has no OIDC or keyless "
                             + "deploy, so this is the only thing there is.")
            } header: {
                Text("Tokens")
            } footer: {
                Text("Stored in the macOS login keychain, never in the repository and never in "
                     + "Keel's own files.")
                    .font(K.F.small).foregroundStyle(K.C.dim)
            }

            Section {
                AwsSso(client: client) { Task { await refresh() } }
            } header: {
                Text("AWS Identity Center")
            } footer: {
                // A profile Keel writes rather than a key it holds: the credential is minted by
                // `aws sso login` and lives in the CLI's own cache.
                Text("Writes a named profile and signs in with `aws sso login`. Keel never asks "
                     + "for an access key and never stores one.")
                    .font(K.F.small).foregroundStyle(K.C.dim)
            }

            if !log.isEmpty {
                Section("Output") {
                    ScrollView {
                        Text(log).font(.system(size: 10.5, design: .monospaced))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .textSelection(.enabled)
                    }
                    .frame(height: 160)
                }
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .frame(maxWidth: .infinity, alignment: .leading)
        .task {
            await refresh()
            stored = try? await client.get("/api/connections")
        }
    }

    private func refresh() async {
        tools = (try? await client.get("/api/cli")) ?? []
    }

    /// Install and login both stream, because these commands print things you have to act on —
    /// `gh` shows a one-time code for the browser.
    private func stream(_ path: String, _ query: [String: String], _ id: String) {
        busy = id
        log = ""
        Task {
            defer { busy = nil }
            do {
                for try await e in client.events(path, query) {
                    switch e.name {
                    case "line", "fatal": log += e.data + "\n"
                    case "done": await refresh()
                    default: break
                    }
                }
            } catch {
                log += error.localizedDescription
            }
        }
    }
}


/// One credential: paste it, Keel verifies it against the provider before storing it.
///
/// Verified rather than merely saved, because a token that is wrong is otherwise discovered
/// halfway through a deploy.
private struct TokenRow: View {
    let client: Client
    let label: String
    let stored: Bool
    let path: String
    let help: String

    @State private var token = ""
    @State private var status: String?
    @State private var failed = false
    @State private var confirming = false

    struct TokenBody: Encodable { var token: String }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(label).font(K.F.body.weight(.medium)).foregroundStyle(K.C.text)
                    .frame(width: 90, alignment: .leading)
                SecureField(stored ? "stored" : "token", text: $token).field()
                Button("Connect") { connect() }
                    .buttonStyle(QuietButton(tone: K.C.accent))
                    .disabled(token.trimmingCharacters(in: .whitespaces).isEmpty)
                if stored {
                    // Confirmed: this removes a credential from the keychain, and the button
                    // beside it is the one that adds one.
                    Button("Disconnect") { confirming = true }
                        .buttonStyle(QuietButton(tone: K.C.del))
                        .alert("Disconnect \(label)?", isPresented: $confirming) {
                            Button("Disconnect", role: .destructive) { disconnect() }
                            Button("Cancel", role: .cancel) {}
                        } message: {
                            Text("Removes the stored token from the login keychain. Anything that "
                                 + "needed it stops working until a new one is connected.")
                        }
                }
            }
            if let status {
                Text(status).font(K.F.small).foregroundStyle(failed ? K.C.del : K.C.add)
            }
            Text(help).font(K.F.small).foregroundStyle(K.C.dim)
        }
    }

    struct ProviderBody: Encodable { var provider: String }

    private func disconnect() {
        Task {
            do {
                _ = try await client.post("/api/disconnect",
                                          body: ProviderBody(provider: label.lowercased()),
                                          as: Bool.self)
                status = "Disconnected."
                failed = false
            } catch {
                status = error.localizedDescription
                failed = true
            }
        }
    }

    private func connect() {
        Task {
            do {
                // The response shape differs per provider, so only success matters here.
                _ = try await client.post(path, body: TokenBody(token: token), as: JSONValue.self)
                status = "Connected."
                failed = false
                token = ""
            } catch {
                status = error.localizedDescription
                failed = true
            }
        }
    }
}


/// The one AWS login Keel can actually drive.
///
/// `aws configure` prompts for a key and a secret, which Keel hosts in a terminal rather than
/// reimplementing. Identity Center is different: it is a form followed by a browser handshake the
/// CLI runs itself, so it can be filled in here.
private struct AwsSso: View {
    let client: Client
    let done: () -> Void

    @State private var startURL = ""
    @State private var ssoRegion = "us-east-1"
    @State private var account = ""
    @State private var role = ""
    @State private var profile = "keel"
    @State private var status: String?
    @State private var failed = false

    struct Setup: Encodable {
        var start_url: String
        var sso_region: String
        var account: String
        var role: String
        var profile: String
        var region: String
    }
    struct Configured: Decodable { var profile: String }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.half) {
            field("Start URL", "https://d-1234567890.awsapps.com/start", $startURL)
            field("Identity Center region", "us-east-1", $ssoRegion)
            field("Account", "123456789012", $account)
            field("Role", "AdministratorAccess", $role)
            field("Profile", "keel", $profile)

            HStack {
                Spacer()
                Button("Set up") { configure() }
                    .buttonStyle(QuietButton(tone: K.C.accent))
                    .disabled(startURL.isEmpty || account.isEmpty || role.isEmpty)
            }
            if let status {
                Text(status).font(K.F.small).foregroundStyle(failed ? K.C.del : K.C.add)
            }
        }
    }

    private func field(_ label: String, _ hint: String, _ value: Binding<String>) -> some View {
        HStack {
            Text(label).font(K.F.small).foregroundStyle(K.C.dim).frame(width: 150, alignment: .leading)
            TextField(hint, text: value).field().font(K.F.small)
        }
    }

    private func configure() {
        Task {
            do {
                let made: Configured = try await client.post(
                    "/api/aws/sso",
                    body: Setup(start_url: startURL, sso_region: ssoRegion, account: account,
                                role: role, profile: profile, region: ssoRegion))
                status = "Profile `\(made.profile)` written. Sign in from the row above."
                failed = false
                done()
            } catch {
                status = error.localizedDescription
                failed = true
            }
        }
    }
}


/// One tool: its state, and every way of fixing it — the command Keel can run, the installer
/// you can download, and the command to paste when Keel cannot drive the flow.
private struct ToolCard: View {
    let tool: ConnectionsSettings.Tool
    let busy: Bool
    let anyBusy: Bool
    let install: () -> Void
    let login: () -> Void

    /// Where the official installer lives, for the people who would rather click than brew.
    private static let installers: [String: String] = [
        "gh": "https://cli.github.com",
        "wrangler": "https://developers.cloudflare.com/workers/wrangler/install-and-update/",
        "gcloud": "https://cloud.google.com/sdk/docs/install",
        "kubectl": "https://kubernetes.io/docs/tasks/tools/install-kubectl-macos/",
        "docker": "https://www.docker.com/products/docker-desktop/",
        "aws": "https://aws.amazon.com/cli/",
        "tailscale": "https://tailscale.com/download/mac",
        "bun": "https://bun.sh",
        "node": "https://nodejs.org/en/download",
        "claude": "https://docs.claude.com/en/docs/claude-code/setup",
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            HStack(spacing: K.S.sm) {
                Pill(text: tool.authenticated ? "OK" : (tool.installed ? "SIGN IN" : "MISSING"),
                     tone: tool.authenticated ? .good : (tool.installed ? .warn : .neutral))
                Text(tool.label).font(K.F.body.weight(.medium)).foregroundStyle(K.C.text)
                Spacer()
                if let v = tool.version { Text(v).font(K.F.mono(10)).foregroundStyle(K.C.faint).lineLimit(1) }
            }
            Text(tool.identity ?? (tool.installed ? "not connected" : "not installed"))
                .font(K.F.small).foregroundStyle(K.C.dim).lineLimit(1)
            if let why = tool.blocked {
                Text(why).font(K.F.micro).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: K.S.sm) {
                if !tool.installed {
                    Button(busy ? "Installing…" : "Install") { install() }
                        .buttonStyle(QuietButton(tone: K.C.accent)).disabled(anyBusy)
                } else if !tool.authenticated, tool.setup?.isEmpty ?? true {
                    Button(busy ? "Signing in…" : "Sign in") { login() }
                        .buttonStyle(QuietButton(tone: K.C.accent)).disabled(anyBusy)
                }
                if let page = Self.installers[tool.id], let url = URL(string: page) {
                    Button(tool.installed ? "Website" : "Download installer") {
                        NSWorkspace.shared.open(url)
                    }
                    .buttonStyle(QuietButton())
                }
            }
            if !tool.authenticated, tool.installed, let setup = tool.setup, !setup.isEmpty {
                // A flow Keel cannot drive — a password prompt, a browser handshake — is
                // offered as the command to run rather than as a button that would do nothing.
                VStack(alignment: .leading, spacing: K.S.xxs) {
                    Text(setup).font(K.F.code).textSelection(.enabled)
                        .padding(.horizontal, K.S.half).padding(.vertical, 3)
                        .background(K.C.well, in: RoundedRectangle(cornerRadius: 3))
                    Text("run this in the terminal").font(K.F.micro).foregroundStyle(K.C.faint)
                }
            }
        }
        .padding(K.S.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
    }
}
