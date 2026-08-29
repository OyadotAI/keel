import AppKit
import SwiftUI

/// Starting something, rather than opening something that exists.
///
/// Two ways in, because they are genuinely different jobs: scaffold a new project onto the golden
/// path, or clone one you already have on GitHub. Both end the same way — a directory on this
/// disk, opened.
struct StartProject: View {
    let client: Client
    let onOpened: (String) -> Void

    @State private var mode = Mode.new
    @State private var name = ""
    @State private var parent = "~/Dev"
    @State private var template = "app"
    @State private var repos: [Repo] = []
    @State private var chosen: Repo?
    @State private var busy = false
    @State private var error: String?

    enum Mode: String, CaseIterable { case new = "New project", clone = "Clone from GitHub" }

    struct Repo: Decodable, Identifiable, Hashable {
        var name: String
        var full_name: String?
        var clone_url: String?
        var updated_at: String?
        var id: String { full_name ?? name }
    }

    struct NewProject: Encodable { var parent: String; var name: String; var template: String }
    struct CloneBody: Encodable { var clone_url: String; var name: String; var parent: String? }
    struct Created: Decodable { var path: String }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Picker("", selection: $mode) {
                ForEach(Mode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented).labelsHidden()

            switch mode {
            case .new: newForm
            case .clone: cloneForm
            }

            if let error {
                Text(error).font(.system(size: 11)).foregroundStyle(.red)
                    .frame(maxWidth: 420, alignment: .leading)
            }
        }
        .padding(20)
        .frame(width: 480)
        .task { await loadRepos() }
    }

    private var newForm: some View {
        VStack(alignment: .leading, spacing: 10) {
            LabeledContent("Name") {
                TextField("my-project", text: $name).textFieldStyle(.roundedBorder)
            }
            LabeledContent("In") {
                HStack {
                    TextField("~/Dev", text: $parent).textFieldStyle(.roundedBorder)
                    Button("Choose…") { chooseParent() }.controlSize(.small)
                }
            }
            Picker("Template", selection: $template) {
                Text("Full stack").tag("app")
                Text("Agent scaffolding only").tag("empty")
            }

            Text(template == "app"
                 ? "Next.js and Hono on Cloudflare, two environments, green from the first commit. "
                   + "The frontend reaches the backend through a service binding, so the call never "
                   + "leaves Cloudflare."
                 : "A CLAUDE.md, a gate and the agent scaffolding — no application code.")
                .font(.system(size: 11)).foregroundStyle(.secondary)

            Button(busy ? "Creating…" : "Create") { create() }
                .disabled(busy || name.trimmingCharacters(in: .whitespaces).isEmpty)
        }
    }

    private var cloneForm: some View {
        VStack(alignment: .leading, spacing: 10) {
            if repos.isEmpty {
                Text("Cloning uses the GitHub CLI. Install and sign in to `gh` under Connections, "
                     + "then reopen this.")
                    .font(.system(size: 11.5)).foregroundStyle(.secondary)
            } else {
                List(repos, selection: $chosen) { r in
                    HStack {
                        Text(r.full_name ?? r.name).font(.system(size: 11, design: .monospaced))
                        Spacer()
                        Text((r.updated_at ?? "").prefix(10))
                            .font(.system(size: 10)).foregroundStyle(.tertiary)
                    }
                    .tag(r)
                }
                .frame(height: 220)

                LabeledContent("Into") {
                    TextField("~/Dev", text: $parent).textFieldStyle(.roundedBorder)
                }
                Text("Cloned with your `gh` credentials over HTTPS, so no SSH key is needed.")
                    .font(.system(size: 11)).foregroundStyle(.secondary)

                Button(busy ? "Cloning…" : "Clone") { clone() }
                    .disabled(busy || chosen == nil)
            }
        }
    }

    private func chooseParent() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.prompt = "Choose"
        if panel.runModal() == .OK, let url = panel.url { parent = url.path }
    }

    private func loadRepos() async {
        repos = (try? await client.get("/api/github/repos")) ?? []
    }

    private func create() {
        busy = true
        Task {
            defer { busy = false }
            do {
                let made: Created = try await client.post(
                    "/api/project/new",
                    body: NewProject(parent: parent,
                                     name: name.trimmingCharacters(in: .whitespaces),
                                     template: template))
                onOpened(made.path)
            } catch { self.error = error.localizedDescription }
        }
    }

    private func clone() {
        guard let r = chosen, let url = r.clone_url else { return }
        busy = true
        Task {
            defer { busy = false }
            do {
                let made: Created = try await client.post(
                    "/api/github/clone",
                    body: CloneBody(clone_url: url, name: r.name, parent: parent))
                onOpened(made.path)
            } catch { self.error = error.localizedDescription }
        }
    }
}
