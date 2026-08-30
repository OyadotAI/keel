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
        VStack(alignment: .leading, spacing: 0) {
            // What is wrong, first. A list of nine rows where two matter is a list you read nine
            // times to find the two.
            SettingsSection("Status") {
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
                            .font(K.F.micro).foregroundStyle(K.C.add)
                            .accessibilityHidden(true)
                        Text("All \(tools.count) tools installed and signed in.")
                            .font(K.F.small).foregroundStyle(K.C.dim)
                    }
                } else {
                    HStack(alignment: .top, spacing: K.S.sm) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(K.F.micro).foregroundStyle(K.C.warn)
                            .accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: K.S.xxs) {
                            Text(broken.map(\.label).joined(separator: ", "))
                                .font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                                .fixedSize(horizontal: false, vertical: true)
                            Text(broken.count == 1
                                 ? "is not ready. Nothing that needs it will work."
                                 : "are not ready. Nothing that needs them will work.")
                                .font(K.F.small).foregroundStyle(K.C.dim)
                        }
                    }
                }
            }

            // One row per tool, the broken ones first. A grid of cards had ragged heights and
            // a command you had to retype; a row has the state, the account, and the button.
            SettingsSection("Command-line tools") {
                boxed {
                    ForEach(tools.sorted {
                        ($0.authenticated ? 1 : 0, $0.label) < ($1.authenticated ? 1 : 0, $1.label)
                    }) { t in
                        ToolRow(tool: t, busy: busy == t.id, anyBusy: busy != nil,
                                install: { stream("/api/cli/install", ["id": t.id], t.id) },
                                login: { stream("/api/cli/login", ["id": t.id], t.id) })
                        if t.id != tools.last?.id { Hairline() }
                    }
                }
            }

            // Tokens and the one AWS flow Keel can drive, as rows in the same list shape as the
            // tools above. The explanations are tooltips; the row says connected or not and
            // offers the one thing to do about it.
            SettingsSection(
                "Credentials",
                note: "They live in the macOS login keychain or the CLI's own cache — never in the "
                    + "repository, never in Keel's files."
            ) {
                boxed {
                    TokenRow(client: client, label: "GitHub token", stored: stored?.github != nil,
                             path: "/api/connect/github",
                             help: "A personal access token, kept in the login keychain. Only "
                                 + "needed for what `gh` cannot do for you.")
                    Hairline()
                    TokenRow(client: client, label: "Cloudflare token",
                             stored: stored?.cloudflare != nil,
                             path: "/api/connect/cloudflare",
                             help: "A scoped, rotatable API token, kept in the login keychain. "
                                 + "Cloudflare has no keyless deploy, so this is the only way.")
                    Hairline()
                    AwsSso(client: client) { Task { await refresh() } }
                }
            }

            if !log.isEmpty {
                SettingsSection("Output") {
                    ScrollView {
                        Text(log).font(K.F.codeTiny).foregroundStyle(K.C.dim)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .textSelection(.enabled)
                            .padding(K.S.sm)
                    }
                    .frame(height: 160)
                    .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.md))
                    .overlay(RoundedRectangle(cornerRadius: K.R.md)
                        .stroke(K.C.line, lineWidth: 1))
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task {
            await refresh()
            stored = try? await client.get("/api/connections")
        }
    }

    /// The one place in Settings that draws a box: a list of rows that belong together and each
    /// carry their own controls, which needs an edge to read as a list rather than as prose.
    @ViewBuilder
    private func boxed<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(spacing: 0) { content() }
            .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.md))
            .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
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

    @State private var editing = false

    var body: some View {
        HStack(spacing: K.S.sm) {
            Pill(text: stored ? "OK" : "NONE", tone: stored ? .good : .neutral)
                .frame(width: 58, alignment: .leading)
            Text(label).font(K.F.body.weight(.medium)).foregroundStyle(K.C.text)
                .frame(width: 110, alignment: .leading)
            Text(status ?? (stored ? "in the keychain" : "not connected"))
                .font(K.F.small)
                .foregroundStyle(failed ? K.C.del : (stored ? K.C.dim : K.C.faint))
                .lineLimit(1)
            Spacer(minLength: K.S.sm)
            if editing {
                SecureField("paste the token", text: $token).field().frame(width: 220)
                    .onSubmit { connect() }
                Button("Save") { connect() }
                    .buttonStyle(QuietButton(tone: K.C.accent))
                    .disabled(token.trimmingCharacters(in: .whitespaces).isEmpty)
                Button("Cancel") { editing = false; token = "" }.buttonStyle(QuietButton())
            } else {
                Button(stored ? "Replace" : "Add token") { editing = true }
                    .buttonStyle(QuietButton(tone: stored ? K.C.dim : K.C.accent))
                if stored {
                    // Confirmed: this removes a credential from the keychain.
                    Button("Disconnect") { confirming = true }
                        .buttonStyle(QuietButton(tone: K.C.del))
                        .alert("Disconnect \(label)?", isPresented: $confirming) {
                            Button("Disconnect", role: .destructive) { disconnect() }
                            Button("Cancel", role: .cancel) {}
                        } message: {
                            Text("Removes the stored token from the login keychain. Anything "
                                 + "that needed it stops working until a new one is connected.")
                        }
                }
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .help(help)
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
                editing = false
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
    @State private var ssoRegion = ""
    @State private var account = ""
    @State private var role = ""
    @State private var profile = ""
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

    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            HStack(spacing: K.S.sm) {
                Pill(text: "SSO", tone: .neutral).frame(width: 58, alignment: .leading)
                Text("AWS Identity Center").font(K.F.body.weight(.medium)).foregroundStyle(K.C.text)
                    .frame(width: 110, alignment: .leading)
                Text(status ?? "writes a profile, then `aws sso login` signs in")
                    .font(K.F.small).foregroundStyle(failed ? K.C.del : K.C.faint).lineLimit(1)
                Spacer(minLength: K.S.sm)
                Button(open ? "Hide" : "Set up…") { withAnimation(K.M.quick) { open.toggle() } }
                    .buttonStyle(QuietButton(tone: open ? K.C.dim : K.C.accent))
            }
            if open {
                // Two columns, the labels inside the fields. Five stacked rows with a label
                // column each was the tallest thing on the page.
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())],
                          alignment: .leading, spacing: K.S.sm) {
                    TextField("Start URL  (https://d-….awsapps.com/start)", text: $startURL).field()
                    TextField("Identity Center region  (us-east-1)", text: $ssoRegion).field()
                    TextField("Account id", text: $account).field()
                    TextField("Role  (e.g. AdministratorAccess)", text: $role).field()
                    TextField("Profile name  (keel)", text: $profile).field()
                    HStack {
                        Spacer()
                        Button("Write profile and sign in") { configure() }
                            .buttonStyle(QuietButton(tone: K.C.accent))
                            .disabled(startURL.isEmpty || account.isEmpty || role.isEmpty)
                    }
                }
                .font(K.F.small)
                .padding(.leading, 58 + 110 + 2 * K.S.sm)
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .help("Keel writes a named AWS profile and signs in with `aws sso login`. It never asks "
              + "for an access key and never stores one.")
    }

    private func configure() {
        Task {
            do {
                let made: Configured = try await client.post(
                    "/api/aws/sso",
                    body: Setup(start_url: startURL,
                                sso_region: ssoRegion.isEmpty ? "us-east-1" : ssoRegion,
                                account: account, role: role,
                                profile: profile.isEmpty ? "keel" : profile,
                                region: ssoRegion.isEmpty ? "us-east-1" : ssoRegion))
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


/// One tool: its state, who is signed in, and the one button that fixes it.
private struct ToolRow: View {
    let tool: ConnectionsSettings.Tool
    let busy: Bool
    let anyBusy: Bool
    let install: () -> Void
    let login: () -> Void
    @State private var copied = false

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

    /// `gh version 2.87.3 (2026-…)` → `2.87.3`.
    private var version: String? {
        guard let v = tool.version,
              let m = v.range(of: #"\d+\.\d+(\.\d+)?"#, options: .regularExpression) else { return tool.version }
        return String(v[m])
    }

    private var setup: String? { tool.installed && !tool.authenticated ? tool.setup.flatMap { $0.isEmpty ? nil : $0 } : nil }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            HStack(spacing: K.S.sm) {
                Pill(text: tool.authenticated ? "OK" : (tool.installed ? "SIGN IN" : "MISSING"),
                     tone: tool.authenticated ? .good : (tool.installed ? .warn : .neutral))
                    .frame(width: 58, alignment: .leading)
                Text(tool.label).font(K.F.body.weight(.medium)).foregroundStyle(K.C.text)
                    .frame(width: 110, alignment: .leading)
                Text(tool.identity ?? (tool.installed ? "not signed in" : "not installed"))
                    .font(K.F.small).foregroundStyle(tool.authenticated ? K.C.dim : K.C.faint)
                    .lineLimit(1).truncationMode(.middle)
                Spacer(minLength: K.S.sm)
                if let version { Text(version).font(K.F.codeTiny).foregroundStyle(K.C.faint) }
                actions
            }
            if let why = tool.blocked, !tool.authenticated {
                Text(why).font(K.F.micro).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 58 + 110 + 2 * K.S.sm)
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
    }

    /// The one thing to do about this row. Install, sign in, or run the command — as a button
    /// that runs it in Keel's terminal, not a string to retype.
    @ViewBuilder
    private var actions: some View {
        HStack(spacing: K.S.xs) {
            if !tool.installed {
                Button(busy ? "Installing…" : "Install") { install() }
                    .buttonStyle(QuietButton(tone: K.C.accent)).disabled(anyBusy)
                if let page = Self.installers[tool.id], let url = URL(string: page) {
                    Button("Download") { NSWorkspace.shared.open(url) }.buttonStyle(QuietButton())
                }
            } else if !tool.authenticated {
                if let setup {
                    Button("Run in Terminal") {
                        NotificationCenter.default.post(name: .keelRunInTerminal, object: setup)
                    }
                    .buttonStyle(QuietButton(tone: K.C.accent))
                    .help(setup)
                    Button(copied ? "Copied" : "Copy") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(setup, forType: .string)
                        copied = true
                    }
                    .buttonStyle(QuietButton())
                } else {
                    Button(busy ? "Signing in…" : "Sign in") { login() }
                        .buttonStyle(QuietButton(tone: K.C.accent)).disabled(anyBusy)
                }
            } else if let page = Self.installers[tool.id], let url = URL(string: page) {
                Button { NSWorkspace.shared.open(url) } label: {
                    Image(systemName: "arrow.up.right.square").font(K.F.tiny)
                        .frame(width: 18, height: 16).contentShape(Rectangle())
                }
                .buttonStyle(.plain).foregroundStyle(K.C.faint)
                .hint("\(tool.label) website")
            }
        }
    }
}
