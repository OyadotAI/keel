import SwiftUI

/// Stream closure is not success. Keep fatal errors sticky even if a later exit code is zero.
struct SetupCommandResult {
    private(set) var finished = false
    private var error: String?
    mutating func receive(_ event: Client.Event) {
        if event.name == "fatal" { error = event.data.isEmpty ? "Setup failed. Try again." : event.data }
        if event.name == "done" {
            finished = true
            if event.data != "0" { error = "Setup exited with code \(event.data). Check the output and retry." }
        }
    }
    var failure: String? { error ?? (finished ? nil : "Setup disconnected before it finished. Check the output and retry.") }
    var succeeded: Bool { finished && error == nil }
    static func credentialProvider(for path: String) -> String { String(path.split(separator: "/").last ?? "") }
    static func awsConfigureCommand(profile: String) -> String {
        profile.isEmpty ? "aws configure" : "aws configure --profile '" + profile.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}

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
    @State private var failure: String?
    @State private var claude: ClaudeSetup
    @State private var choosingContext = false
    @State private var refreshing = false
    @State private var outputTool: String?
    @State private var awsProfile = ""

    private let onToolsChanged: ([Tool]) -> Void

    init(client: Client, tools: [Tool] = [], claudeStatus: ClaudeSetup.Status? = nil,
         onToolsChanged: @escaping ([Tool]) -> Void = { _ in }) {
        self.client = client
        self.onToolsChanged = onToolsChanged
        _tools = State(initialValue: tools)
        _claude = State(initialValue: ClaudeSetup(client: client, status: claudeStatus))
    }

    struct Stored: Decodable {
        var github: JSONValue?
        var cloudflare: JSONValue?
        var github_via_gh: Bool?
        var github_stored: Bool?
        var cloudflare_stored: Bool?
        var hasGitHubToken: Bool { github_stored ?? (github != nil && github_via_gh != true) }
        var hasCloudflareToken: Bool { cloudflare_stored ?? (cloudflare != nil) }
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
        var install_cmd: String?
        var manual: String?
        var reconnect: String?
        var profiles: [String]?

        var statusLabel: String {
            if !installed { return "MISSING" }
            if authenticated { return "READY" }
            if reconnect != nil { return "SIGN IN" }
            if id == "kubectl", identity != nil { return "CHECK" }
            return ["kubectl", "docker", "tailscale"].contains(id) ? "SET UP" : "SIGN IN"
        }
    }

    private var broken: [Tool] { tools.filter { !$0.installed || !$0.authenticated } }
    private var orderedTools: [Tool] {
        tools.sorted { ($0.authenticated ? 1 : 0, $0.label) < ($1.authenticated ? 1 : 0, $1.label) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSection("Agent setup", note: "Install Claude Code and connect your account without leaving Keel.") {
                ClaudeSetupCard(setup: claude)
            }
            // What is wrong, first. A list of nine rows where two matter is a list you read nine
            // times to find the two.
            SettingsSection("Status") {
                if let failure {
                    Text(failure).font(K.F.small).foregroundStyle(K.C.del)
                    Button("Try again") { Task { await refresh() } }.buttonStyle(QuietButton())
                } else if tools.isEmpty {
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
                        Text("All \(tools.count) optional tools ready.")
                            .font(K.F.small).foregroundStyle(K.C.dim)
                    }
                } else {
                    HStack(alignment: .top, spacing: K.S.sm) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(K.F.micro).foregroundStyle(K.C.warn)
                            .accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: K.S.xxs) {
                            Text("\(tools.count - broken.count) of \(tools.count) optional tools ready")
                                .font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                            Text("Install the tools your project needs. You do not need all of them to start coding.")
                                .font(K.F.small).foregroundStyle(K.C.dim)
                        }
                    }
                }
            }

            Button(refreshing ? "Checking tools…" : "Refresh tools") { Task { await refresh() } }
                .buttonStyle(QuietButton()).disabled(refreshing || busy != nil)

            // One row per tool, the broken ones first. A grid of cards had ragged heights and
            // a command you had to retype; a row has the state, the account, and the button.
            SettingsSection("Command-line tools", note: "Installers show the command before running it. Sign-in uses each tool's own account flow.") {
                boxed {
                    ForEach(orderedTools) { t in
                        ToolRow(tool: t, busy: busy == t.id, anyBusy: busy != nil,
                                install: { stream("/api/cli/install", ["id": t.id], t.id) },
                                login: { stream("/api/cli/login", ["id": t.id], t.id) },
                                chooseContext: { choosingContext = true },
                                checking: refreshing, testConnection: { Task { await refresh() } },
                                reconnectCloud: { stream("/api/cli/login", ["id": "gcloud"], t.id) },
                                configureAws: {
                                    NotificationCenter.default.post(name: .keelRunInTerminal,
                                        object: SetupCommandResult.awsConfigureCommand(profile: awsProfile))
                                })
                        if t.id == "aws", t.installed {
                            awsProfileControls(t).padding(K.S.md)
                        }
                        if outputTool == t.id { setupOutput.padding(K.S.md) }
                        if t.id != orderedTools.last?.id { Hairline() }
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
                    if stored?.github_via_gh == true {
                        Text("GitHub is connected through GitHub CLI. No Keel-managed token is needed.")
                            .font(K.F.small).foregroundStyle(K.C.dim).padding(K.S.md)
                    }
                    TokenRow(client: client, label: "GitHub token", stored: stored?.hasGitHubToken == true,
                             path: "/api/connect/github",
                             help: "A personal access token, kept in the login keychain. Only "
                                 + "needed for what `gh` cannot do for you.")
                    Hairline()
                    TokenRow(client: client, label: "Cloudflare token",
                             stored: stored?.hasCloudflareToken == true,
                             path: "/api/connect/cloudflare",
                             help: "A scoped, rotatable API token, kept in the login keychain. "
                                 + "Optional when Wrangler is signed in with its browser login.")
                    Hairline()
                    AwsSso(client: client) { profile in
                        awsProfile = profile
                        Task { await refresh() }
                    }
                }
            }

        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .sheet(isPresented: $choosingContext, onDismiss: { Task { await refresh() } }) {
            KubernetesContextSheet(client: client) { choosingContext = false }
        }
        .task {
            async let provider: () = claude.refresh()
            await refresh()
            await provider
        }
        .onDisappear { claude.cancel() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            if busy == nil { Task { await refresh(); await claude.refresh() } }
        }
    }

    private var setupOutput: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            if busy != nil {
                HStack { ProgressView().controlSize(.small); Text("Setup is running…").font(K.F.small) }
            }
            ForEach(Array(ClaudeSetup.loginLinks(in: log).prefix(6)), id: \.self) { url in
                Link("Open in browser · \(url.host ?? "sign in") ↗", destination: url).font(K.F.small)
            }
            ScrollView {
                Text(log).font(K.F.codeTiny).foregroundStyle(K.C.dim)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled).padding(K.S.sm)
            }
            .frame(height: 160)
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.md))
            .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
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
        guard !refreshing else { return }
        refreshing = true
        defer { refreshing = false }
        do {
            tools = try await client.get("/api/cli", awsProfile.isEmpty ? [:] : ["aws_profile": awsProfile])
            onToolsChanged(tools)
            stored = try? await client.get("/api/connections")
            failure = nil
        } catch { failure = "Could not check tools: \(error.localizedDescription)" }
    }

    private func awsProfileControls(_ tool: Tool) -> some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            Picker("AWS profile", selection: $awsProfile) {
                Text("Default credential chain").tag("")
                ForEach(tool.profiles ?? [], id: \.self) { Text($0).tag($0) }
            }
            .disabled(refreshing || busy != nil)
            .onChange(of: awsProfile) { Task { await refresh() } }
            HStack {
                Button("SSO sign in") {
                    stream("/api/cli/login", ["id": "aws", "profile": awsProfile], "aws")
                }.buttonStyle(QuietButton(tone: K.C.accent))
                    .disabled(awsProfile.isEmpty || busy != nil || refreshing)
                Button("Test selected profile") { Task { await refresh() } }
                    .buttonStyle(QuietButton()).disabled(busy != nil || refreshing)
            }
            Text("Selection is used for this check and SSO sign-in. It does not change AWS_PROFILE in your terminals or agents. For access-key profiles, use Configure credentials above.")
                .font(K.F.micro).foregroundStyle(K.C.dim)
        }
    }

    /// Install and login both stream, because these commands print things you have to act on —
    /// `gh` shows a one-time code for the browser.
    private func stream(_ path: String, _ query: [String: String], _ id: String) {
        guard busy == nil else { return }
        busy = id
        outputTool = id
        log = ""
        Task {
            defer { busy = nil }
            var result = SetupCommandResult()
            do {
                for try await e in client.events(path, query) {
                    result.receive(e)
                    switch e.name {
                    case "line", "fatal": log += e.data + "\n"
                    case "done":
                        if e.data != "0" { log += "Setup exited with code \(e.data). Check the output above.\n" }
                        await refresh()
                    default: break
                    }
                }
                if let failure = result.failure { log += failure + "\n" }
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
    @State private var storedOverride: Bool?
    private var connected: Bool { storedOverride ?? stored }
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
            Pill(text: connected ? "OK" : "NONE", tone: connected ? .good : .neutral)
                .frame(width: 58, alignment: .leading)
            Text(label).font(K.F.body.weight(.medium)).foregroundStyle(K.C.text)
                .frame(width: 110, alignment: .leading)
            Text(status ?? (connected ? "in the keychain" : "not connected"))
                .font(K.F.small)
                .foregroundStyle(failed ? K.C.del : (connected ? K.C.dim : K.C.faint))
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
                Button(connected ? "Replace" : "Add token") { editing = true }
                    .buttonStyle(QuietButton(tone: connected ? K.C.dim : K.C.accent))
                if connected {
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
                                          body: ProviderBody(provider: SetupCommandResult.credentialProvider(for: path)),
                                          as: Bool.self)
                status = "Disconnected."
                storedOverride = false
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
                storedOverride = true
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
    let done: (String) -> Void

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
    @State private var saving = false

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
                        Button(saving ? "Saving…" : "Save profile") { configure() }
                            .buttonStyle(QuietButton(tone: K.C.accent))
                            .disabled(saving || startURL.isEmpty || account.count != 12 || role.isEmpty)
                    }
                }
                .font(K.F.small)
                .padding(.leading, 58 + 110 + 2 * K.S.sm)
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .help("Save a named AWS profile, then use SSO sign in in the AWS row. Keel never asks "
              + "for an access key and never stores one.")
    }

    private func configure() {
        guard !saving else { return }
        saving = true
        Task {
            defer { saving = false }
            do {
                let made: Configured = try await client.post(
                    "/api/aws/sso",
                    body: Setup(start_url: startURL,
                                sso_region: ssoRegion.isEmpty ? "us-east-1" : ssoRegion,
                                account: account, role: role,
                                profile: profile.isEmpty ? "keel" : profile,
                                region: ssoRegion.isEmpty ? "us-east-1" : ssoRegion))
                status = "Profile \(made.profile) selected. Use SSO sign in in the AWS row."
                failed = false
                done(made.profile)
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
    let chooseContext: () -> Void
    let checking: Bool
    let testConnection: () -> Void
    let reconnectCloud: () -> Void
    let configureAws: () -> Void
    @State private var copied = false
    @State private var confirmingInstall = false

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
                Pill(text: tool.statusLabel,
                     tone: tool.authenticated ? .good : (tool.installed ? .warn : .neutral))
                    .frame(width: 58, alignment: .leading)
                Text(tool.label).font(K.F.body.weight(.medium)).foregroundStyle(K.C.text)
                    .frame(width: 110, alignment: .leading)
                Text(tool.identity ?? (tool.installed ? (tool.id == "kubectl" ? "no current context" : "not ready") : "not installed"))
                    .font(K.F.small).foregroundStyle(tool.authenticated ? K.C.dim : K.C.faint)
                    .lineLimit(1).truncationMode(.middle)
                Spacer(minLength: K.S.sm)
                if let version { Text(version).font(K.F.codeTiny).foregroundStyle(K.C.faint) }
                actions
            }
            if let why = tool.blocked, !tool.authenticated {
                Text(why).font(K.F.micro).foregroundStyle(K.C.dim)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 58 + 110 + 2 * K.S.sm)
            }
            if tool.installed && tool.reconnect == "gcloud" {
                Button(busy ? "Reconnecting…" : "Reconnect Google Cloud", action: reconnectCloud)
                    .buttonStyle(QuietButton(tone: K.C.accent)).disabled(anyBusy || checking)
                    .padding(.leading, 58 + 110 + 2 * K.S.sm)
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .confirmationDialog("Install \(tool.label)?", isPresented: $confirmingInstall,
                            titleVisibility: .visible) {
            Button("Install \(tool.label)") {
                if let command = tool.install_cmd, command.contains("--cask") {
                    // App installers can request an administrator password. Give them a PTY,
                    // not a pipe with no input, so the person can answer the real system flow.
                    NotificationCenter.default.post(name: .keelRunInTerminal, object: command)
                } else { install() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Keel will run:\n\(tool.install_cmd ?? "the tool's installer")\n\nThis installs software on your Mac, not in the repository.")
        }
    }

    /// The one thing to do about this row. Install, sign in, or run the command — as a button
    /// that runs it in Keel's terminal, not a string to retype.
    @ViewBuilder
    private var actions: some View {
        HStack(spacing: K.S.xs) {
            if !tool.installed {
                if tool.install_cmd != nil {
                    Button(busy ? "Installing…" : "Install") { confirmingInstall = true }
                        .buttonStyle(QuietButton(tone: K.C.accent)).disabled(anyBusy)
                } else {
                    Button("Set up Homebrew") {
                        NotificationCenter.default.post(name: .keelRunInTerminal,
                            object: "/bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\"")
                    }
                    .buttonStyle(QuietButton(tone: K.C.accent)).disabled(anyBusy)
                    .help("Open Homebrew's official installer in Keel's terminal, including its password prompt")
                }
                if let page = tool.manual ?? Self.installers[tool.id], let url = URL(string: page) {
                    Button("Download") { NSWorkspace.shared.open(url) }.buttonStyle(QuietButton())
                }
            } else if tool.id == "kubectl" {
                Button(checking ? "Testing…" : "Test connection", action: testConnection)
                    .buttonStyle(QuietButton()).disabled(anyBusy || checking)
                Button("Choose context…", action: chooseContext)
                    .buttonStyle(QuietButton(tone: K.C.accent)).disabled(anyBusy)
            } else if tool.id == "aws" {
                Button("Configure credentials…", action: configureAws)
                    .buttonStyle(QuietButton()).disabled(anyBusy)
            } else if !tool.authenticated {
                Button(checking ? "Testing…" : "Test connection", action: testConnection)
                    .buttonStyle(QuietButton()).disabled(anyBusy || checking)
                if let setup {
                    Button(tool.id == "docker" ? "Open Docker Desktop" : (tool.id == "tailscale" ? "Connect Tailscale…" : "Run in Terminal")) {
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
