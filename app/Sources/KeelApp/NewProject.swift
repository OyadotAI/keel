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
    /// Where it runs. Containers by default: Docker, kustomize, a cluster — the production
    /// shape. Workers for a project that wants no cluster at all.
    @State private var stack = "stack"
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

    struct NewProject: Encodable { var parent: String; var name: String; var template: String; var notes: String; var pack: String; var patterns: String }
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
        .frame(width: mode == .new ? 1180 : 640)
        .background(K.C.bg)
        .task { await loadRepos() }
    }

    // MARK: New

    @State private var category: Template.Category? = nil

    private var templates: [Template] {
        Template.all.filter { t in
            (category == nil || t.category == category)
                && (query.isEmpty
                    || t.title.localizedCaseInsensitiveContains(query)
                    || t.blurb.localizedCaseInsensitiveContains(query)
                    || t.components.contains { $0.tech.localizedCaseInsensitiveContains(query) || $0.name.localizedCaseInsensitiveContains(query) })
        }
    }

    /// Categories on the left, the list in the middle, the architecture on the right. The
    /// people this is for choose by the shape of the system, so the shape is what they see
    /// before Create — and it is the same text the agent builds to.
    private var newForm: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            HStack(alignment: .top, spacing: K.S.md) {
                categories.frame(width: 180)
                VStack(spacing: K.S.sm) {
                    search("Search \(Template.all.count) templates…")
                    ScrollView {
                        LazyVStack(spacing: 2) {
                            ForEach(templates) { t in
                                let on = template.id == t.id
                                HStack(spacing: K.S.sm) {
                                    Image(systemName: t.icon).font(.system(size: 12))
                                        .foregroundStyle(on ? K.C.accent : K.C.dim).frame(width: 18)
                                    VStack(alignment: .leading, spacing: 1) {
                                        Text(t.title).font(K.F.small.weight(on ? .semibold : .regular))
                                            .foregroundStyle(K.C.text)
                                        if !t.like.isEmpty {
                                            Text("like \(t.like)").font(K.F.micro).foregroundStyle(K.C.accent.opacity(0.9))
                                                .lineLimit(1)
                                        }
                                        Text(t.blurb).font(K.F.micro).foregroundStyle(K.C.faint)
                                            .lineLimit(2).fixedSize(horizontal: false, vertical: true)
                                    }
                                    Spacer(minLength: 0)
                                }
                                .padding(.horizontal, K.S.sm).padding(.vertical, K.S.xs + 1)
                                .background(on ? K.C.accent.opacity(0.10) : .clear,
                                            in: RoundedRectangle(cornerRadius: K.R.sm))
                                .contentShape(Rectangle())
                                .asButton { template = t; if t.wants == nil { attachment = nil } }
                                .accessibilityAddTraits(on ? .isSelected : [])
                            }
                            if templates.isEmpty {
                                Text("Nothing matches.").font(K.F.small).foregroundStyle(K.C.faint).padding(K.S.md)
                            }
                        }
                    }
                }
                .frame(width: 300)
                detail
            }
            .frame(height: 560)

            Hairline()
            footer
        }
    }

    private var categories: some View {
        VStack(alignment: .leading, spacing: 2) {
            categoryRow(nil, "All", "square.grid.2x2", Template.all.count)
            ForEach(Template.Category.allCases) { c in
                categoryRow(c, c.rawValue, c.icon, Template.all.count { $0.category == c })
            }
        }
    }

    private func categoryRow(_ c: Template.Category?, _ title: String, _ icon: String, _ n: Int) -> some View {
        let on = category == c
        return HStack(spacing: K.S.sm) {
            Image(systemName: icon).font(.system(size: 11)).foregroundStyle(on ? K.C.accent : K.C.faint).frame(width: 16)
            Text(title).font(K.F.small.weight(on ? .semibold : .regular)).foregroundStyle(K.C.text).lineLimit(1)
            Spacer()
            Text("\(n)").font(K.F.mono(10)).foregroundStyle(K.C.faint)
        }
        .padding(.horizontal, K.S.sm).padding(.vertical, 5)
        .background(on ? K.C.text.opacity(0.06) : .clear, in: RoundedRectangle(cornerRadius: K.R.sm))
        .contentShape(Rectangle())
        .asButton {
            category = c
            if let first = templates.first, !templates.contains(where: { $0.id == template.id }) { template = first }
        }
    }

    /// The architecture: components, the flow of one request, what is built in, what you get.
    private var detail: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: K.S.md) {
                HStack(spacing: K.S.sm) {
                    Image(systemName: template.icon).font(.system(size: 16)).foregroundStyle(K.C.accent)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(template.title).font(K.F.title).foregroundStyle(K.C.text)
                        if !template.like.isEmpty {
                            Text("like \(template.like)").font(K.F.small).foregroundStyle(K.C.accent)
                        }
                    }
                    Spacer()
                    Text(template.category.rawValue).font(K.F.micro).foregroundStyle(K.C.faint)
                }
                Text(template.blurb).font(K.F.body).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)

                if !template.components.isEmpty {
                    section("Architecture", "square.stack.3d.up")
                    ArchitectureDiagram(template: template)
                }
                if !template.flow.isEmpty {
                    section("Request path", "arrow.right")
                    FlowStrip(steps: template.flow)
                }
                if !template.practices.isEmpty {
                    section("Built in", "checkmark.shield")
                    PracticePills(practices: template.practices)
                }

                section("You get", "shippingbox")
                Flow(spacing: K.S.xs) {
                    ForEach(template.scaffold == "empty"
                            ? ["CLAUDE.md", "gate", "3 reviewer agents"]
                            : stack == "stack"
                                ? ["Next.js image", "Hono image", "compose: Postgres · Redis · nginx", "k8s base + dev/prod", "env → secrets", "CI → ghcr → cluster", "CLAUDE.md + architecture", "3 reviewer agents"]
                                : ["Next.js on Workers", "Hono on Workers", "service binding", "infra/deploy dev/prod", "CLAUDE.md + architecture", "3 reviewer agents"],
                            id: \.self) { item in
                        Text(item).font(K.F.small).foregroundStyle(K.C.dim)
                            .padding(.horizontal, K.S.sm).padding(.vertical, 3)
                            .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.sm))
                            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
                    }
                }

                if let wants = template.wants {
                    HStack(spacing: K.S.sm) {
                        Image(systemName: "paperclip").font(.system(size: 10)).foregroundStyle(K.C.accent)
                        Text(attachment.map { $0.lastPathComponent } ?? "Attach \(wants)")
                            .font(K.F.small).foregroundStyle(attachment == nil ? K.C.dim : K.C.text).lineLimit(1)
                        Spacer()
                        Button(attachment == nil ? "Choose file…" : "Change") { chooseAttachment() }
                            .buttonStyle(QuietButton(tone: K.C.accent))
                    }
                    .padding(K.S.sm)
                    .background(K.C.accent.opacity(0.06), in: RoundedRectangle(cornerRadius: K.R.sm))
                }
            }
            .padding(.trailing, K.S.xs)
        }
        .frame(maxWidth: .infinity)
    }

    private func section(_ title: String, _ icon: String) -> some View {
        HStack(spacing: K.S.xs) {
            Image(systemName: icon).font(.system(size: 10)).foregroundStyle(K.C.faint)
            Text(title.uppercased()).font(.system(size: 10, weight: .semibold)).tracking(0.7).foregroundStyle(K.C.faint)
        }
        .padding(.top, K.S.xs)
    }

    private var footer: some View {
        HStack(spacing: K.S.sm) {
            TextField("project-name", text: $name).field().font(K.F.code).frame(width: 170)
            TextField("~/Dev", text: $parent).field().font(K.F.code).frame(width: 150)
            Button("Choose…") { chooseParent() }.buttonStyle(QuietButton())
            HStack(spacing: 0) {
                ForEach([("stack", "Containers"), ("app", "Cloudflare Workers")], id: \.0) { s, label in
                    let on = stack == s
                    Text(label)
                        .font(K.F.micro.weight(on ? .semibold : .regular))
                        .foregroundStyle(on ? K.C.text : K.C.faint)
                        .padding(.horizontal, K.S.sm).padding(.vertical, 5)
                        .background(RoundedRectangle(cornerRadius: K.R.sm - 1).fill(on ? K.C.raised : .clear).padding(1))
                        .contentShape(Rectangle())
                        .asButton { stack = s }
                        .help(s == "stack"
                              ? "Docker images, docker-compose, nginx, kustomize overlays, CI to GHCR and a cluster"
                              : "Next.js and Hono as Workers with a service binding; no cluster")
                }
            }
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            Spacer()
            Button(busy ? "Creating…" : (template.brief.isEmpty ? "Create" : "Create and start")) { create() }
                .buttonStyle(SendButton())
                .disabled(busy || name.trimmingCharacters(in: .whitespaces).isEmpty
                          || (template.wants != nil && attachment == nil))
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
                                     template: template.scaffold == "empty" ? "empty" : stack,
                                     notes: template.components.isEmpty ? "" : template.architecture,
                                     pack: template.id,
                                     patterns: template.scaffold == "empty" ? Template.patternsDoc : ""))
                Telemetry.track("project_created", ["template": template.id, "stack": stack])
                onOpened(made.path, template.fullBrief, attachment)
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
