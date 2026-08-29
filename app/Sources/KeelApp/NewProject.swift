import AppKit
import SwiftUI

/// Starting something, rather than opening something that exists.
///
/// Two ways in, because they are genuinely different jobs: scaffold a new project from a
/// template, or clone one you already have on GitHub. Both end the same way — a directory on
/// this disk, opened — and a template also hands the agent its first brief.
struct StartProject: View {
    let client: Client
    /// The path opened, the brief to send first (empty for none), and a file to attach with it.
    let onOpened: (String, String, URL?) -> Void

    @State private var mode = Mode.new
    @State private var name = ""
    @State private var parent = "~/Dev"
    @State private var template: Template = Template.all[0]
    @State private var attachment: URL?
    @State private var query = ""
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
        VStack(alignment: .leading, spacing: K.S.md) {
            HStack(spacing: 0) {
                ForEach(Mode.allCases, id: \.self) { m in
                    let on = mode == m
                    Text(m.rawValue)
                        .font(K.F.small.weight(on ? .semibold : .regular))
                        .foregroundStyle(on ? K.C.text : K.C.faint)
                        .padding(.horizontal, K.S.md).padding(.vertical, 5)
                        .background(RoundedRectangle(cornerRadius: K.R.sm - 1)
                            .fill(on ? K.C.raised : .clear).padding(1))
                        .contentShape(Rectangle())
                        .asButton { mode = m; query = "" }
                }
            }
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))

            switch mode {
            case .new: newForm
            case .clone: cloneForm
            }

            if let error {
                Text(error).font(K.F.small).foregroundStyle(K.C.del)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(K.S.xl)
        .frame(width: 640)
        .background(K.C.bg)
        .task { await loadRepos() }
    }

    // MARK: New

    private var templates: [Template] {
        guard !query.isEmpty else { return Template.all }
        return Template.all.filter {
            $0.title.localizedCaseInsensitiveContains(query) || $0.blurb.localizedCaseInsensitiveContains(query)
        }
    }

    private var newForm: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            search("Search templates…")

            // The gallery. Ten of the things people start most, each a scaffold plus the first
            // brief, so "new project" ends with the agent already working on the right thing.
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 180), spacing: K.S.sm)], spacing: K.S.sm) {
                ForEach(templates) { t in
                    let on = template.id == t.id
                    VStack(alignment: .leading, spacing: K.S.xs) {
                        HStack(spacing: K.S.sm) {
                            Image(systemName: t.icon).font(.system(size: 13))
                                .foregroundStyle(on ? K.C.accent : K.C.dim)
                            Text(t.title).font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                        }
                        Text(t.blurb).font(K.F.micro).foregroundStyle(K.C.dim)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(K.S.sm)
                    .frame(maxWidth: .infinity, minHeight: 72, alignment: .topLeading)
                    .background(on ? K.C.accent.opacity(0.10) : K.C.raised,
                                in: RoundedRectangle(cornerRadius: K.R.md))
                    .overlay(RoundedRectangle(cornerRadius: K.R.md)
                        .stroke(on ? K.C.accent : K.C.line, lineWidth: 1))
                    .contentShape(Rectangle())
                    .asButton { template = t; if t.wants == nil { attachment = nil } }
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }

            if let wants = template.wants {
                HStack(spacing: K.S.sm) {
                    Image(systemName: "paperclip").font(.system(size: 10)).foregroundStyle(K.C.accent)
                    Text(attachment.map { $0.lastPathComponent } ?? "Attach \(wants)")
                        .font(K.F.small).foregroundStyle(attachment == nil ? K.C.dim : K.C.text)
                        .lineLimit(1)
                    Spacer()
                    Button(attachment == nil ? "Choose file…" : "Change") { chooseAttachment() }
                        .buttonStyle(QuietButton(tone: K.C.accent))
                }
                .padding(K.S.sm)
                .background(K.C.accent.opacity(0.06), in: RoundedRectangle(cornerRadius: K.R.sm))
            }

            HStack(spacing: K.S.sm) {
                TextField("project-name", text: $name).field().font(K.F.code).frame(width: 200)
                TextField("~/Dev", text: $parent).field().font(K.F.code)
                Button("Choose…") { chooseParent() }.buttonStyle(QuietButton())
            }

            HStack {
                Text(template.scaffold == "app"
                     ? "Next.js + Hono on Cloudflare, two environments, green from the first commit."
                     : "A CLAUDE.md, a gate and the agent scaffolding — no application code.")
                    .font(K.F.micro).foregroundStyle(K.C.faint)
                Spacer()
                Button(busy ? "Creating…" : (template.brief.isEmpty ? "Create" : "Create and start")) { create() }
                    .buttonStyle(SendButton())
                    .disabled(busy || name.trimmingCharacters(in: .whitespaces).isEmpty
                              || (template.wants != nil && attachment == nil))
            }
        }
    }

    // MARK: Clone

    private var hits: [Repo] {
        guard !query.isEmpty else { return repos }
        return repos.filter { ($0.full_name ?? $0.name).localizedCaseInsensitiveContains(query) }
    }

    private var cloneForm: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            if repos.isEmpty {
                Text("Cloning uses the GitHub CLI. Install and sign in to `gh` under Settings › Tools, "
                     + "then reopen this.")
                    .font(K.F.small).foregroundStyle(K.C.dim)
            } else {
                search("Search \(repos.count) repositories…")
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(hits) { r in
                            HoverRow(selected: chosen == r) {
                                HStack {
                                    Text(r.full_name ?? r.name).font(K.F.code).foregroundStyle(K.C.text)
                                        .lineLimit(1)
                                    Spacer()
                                    Text((r.updated_at ?? "").prefix(10)).font(K.F.mono(10))
                                        .foregroundStyle(K.C.faint)
                                }
                            } action: { chosen = r }
                        }
                        if hits.isEmpty {
                            Text("Nothing matches.").font(K.F.small).foregroundStyle(K.C.faint).padding(K.S.md)
                        }
                    }
                }
                .frame(height: 260)
                .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.md))
                .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))

                HStack(spacing: K.S.sm) {
                    TextField("~/Dev", text: $parent).field().font(K.F.code)
                    Button("Choose…") { chooseParent() }.buttonStyle(QuietButton())
                    Button(busy ? "Cloning…" : "Clone") { clone() }
                        .buttonStyle(SendButton())
                        .disabled(busy || chosen == nil)
                }
                Text("Cloned with your `gh` credentials over HTTPS, so no SSH key is needed.")
                    .font(K.F.micro).foregroundStyle(K.C.faint)
            }
        }
    }

    private func search(_ placeholder: String) -> some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(K.C.faint)
            TextField(placeholder, text: $query).textFieldStyle(.plain).font(K.F.body)
        }
        .padding(.horizontal, K.S.sm).padding(.vertical, K.S.xs + 1)
        .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
    }

    private func chooseParent() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.prompt = "Choose"
        if panel.runModal() == .OK, let url = panel.url { parent = url.path }
    }

    private func chooseAttachment() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.prompt = "Attach"
        if panel.runModal() == .OK, let url = panel.url { attachment = url }
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
                                     template: template.scaffold))
                Telemetry.track("project_created", ["template": template.id])
                onOpened(made.path, template.brief, attachment)
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
                Telemetry.track("project_cloned")
                onOpened(made.path, "", nil)
            } catch { self.error = error.localizedDescription }
        }
    }
}
