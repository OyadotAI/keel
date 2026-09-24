import SwiftUI

/// Installing a skill means installing the plugin that carries it, which is what `claude` itself
/// does — so this browses the marketplaces already configured rather than inventing a registry.
struct SkillCatalog: View {
    let client: Client
    /// The person's "commit after every turn" setting: when it is on, a skill Keel adds is
    /// committed on its own, so the next turn's checkpoint does not sweep it in under that
    /// turn's prompt.
    var autoCommit = true
    let done: () -> Void

    @State private var suggested: [Entry] = []
    @State private var all: [Entry] = []
    @State private var query = ""
    @State private var installing: String?
    @State private var log = ""
    @State private var loading = true
    @State private var failure: String?
    /// Keel's own optional skills, from their own request: a broken marketplace must not hide them.
    @State private var keel: [KeelSkill] = []
    @State private var keelFailure: String?
    @State private var keelLoading = true

    init(client: Client, catalog: Catalog? = nil, failure: String? = nil,
         keel: [KeelSkill]? = nil, autoCommit: Bool = true, done: @escaping () -> Void) {
        self.client = client
        self.autoCommit = autoCommit
        self.done = done
        _keel = State(initialValue: keel ?? [])
        _keelLoading = State(initialValue: keel == nil)
        _suggested = State(initialValue: catalog?.suggested ?? [])
        _all = State(initialValue: catalog?.all ?? [])
        _loading = State(initialValue: catalog == nil && failure == nil)
        _failure = State(initialValue: failure)
    }

    struct Catalog: Decodable { var suggested: [Entry]; var all: [Entry] }
    struct Entry: Decodable, Identifiable, Sendable {
        var name: String
        var description: String?
        var marketplace: String
        var installed: Bool
        var reason: String?
        var id: String { marketplace + name }
    }

    static func results(suggested: [Entry], all: [Entry], query: String) -> [Entry] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return suggested.isEmpty ? all : suggested }
        return all.filter {
            $0.name.localizedCaseInsensitiveContains(query)
                || ($0.description ?? "").localizedCaseInsensitiveContains(query)
        }
        .prefix(60).map { $0 }
    }

    private var hits: [Entry] { Self.results(suggested: suggested, all: all, query: query) }

    struct KeelCatalog: Decodable { var entries: [KeelSkill] }
    struct KeelSkill: Decodable, Identifiable, Sendable {
        var id: String
        var description: String
        var license: String
        var author: String
        var files: Int
        var bytes: Int
        var script: String?
        /// "project" or "user" when a folder of that name already exists.
        var present: String?
        var python: Bool
    }
    struct Installed: Decodable { var path: String; var files: Int; var committed: String?; var note: String? }

    static func keelHits(_ entries: [KeelSkill], query: String) -> [KeelSkill] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return entries }
        return entries.filter {
            $0.id.localizedCaseInsensitiveContains(query) || $0.description.localizedCaseInsensitiveContains(query)
        }
    }

    /// The row's facts line: licence and author first, because they are why the row may exist.
    static func facts(_ s: KeelSkill) -> String {
        var parts = [s.license, s.author, Int64(s.bytes).formatted(.byteCount(style: .file))]
        if s.script != nil { parts.append("includes a Python script") }
        return parts.joined(separator: " · ")
    }

    /// What the log well says after Add, including whether it was committed and why not.
    static func added(_ r: Installed, asked commit: Bool) -> String {
        let wrote = "Wrote \(r.files) file\(r.files == 1 ? "" : "s") to \(r.path)/"
        let note = r.note.map { $0.isEmpty ? "" : " (\($0))" } ?? ""
        if r.committed != nil { return wrote + " and committed them." + note }
        if commit, let n = r.note, !n.isEmpty { return wrote + " — not committed: " + n }
        return wrote + " — not committed."
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "magnifyingglass").font(K.F.micro)
                    .foregroundStyle(K.C.faint)
                TextField("Search \(all.count) skill\(all.count == 1 ? "" : "s") and plugins…", text: $query)
                    .textFieldStyle(.plain).font(K.F.body)
                Button("Done") { done() }.buttonStyle(QuietButton())
            }
            .padding(K.S.md)
            Hairline()

            keelSection

            if query.isEmpty && !suggested.isEmpty {
                Text("SUGGESTED FOR THIS REPOSITORY")
                    .sectionLabel()
                    .foregroundStyle(K.C.faint)
                    .padding(.horizontal, K.S.md).padding(.top, K.S.sm)
            }

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if loading {
                        HStack { ProgressView().controlSize(.small); Text("Loading skills and plugins…").font(K.F.small) }
                            .padding(K.S.md)
                    } else if let failure {
                        EmptyState(icon: "exclamationmark.triangle", title: "Could not load the catalog",
                                   failure, actionLabel: "Try again") { Task { await refresh() } }
                    } else if hits.isEmpty {
                        EmptyState(icon: "magnifyingglass", title: query.isEmpty ? "No plugins available" : "No matches",
                                   query.isEmpty ? "No marketplaces returned plugins. Refresh after configuring a marketplace in Claude Code." : "Try a different skill or plugin name.",
                                   actionLabel: query.isEmpty ? "Refresh" : "Clear search") {
                            if query.isEmpty { Task { await refresh() } } else { query = "" }
                        }
                    }
                    ForEach(hits) { e in row(e) }
                }
            }
            .frame(height: 340)

            if !log.isEmpty {
                Hairline()
                ScrollView {
                    Text(log).font(K.F.codeSmall).foregroundStyle(K.C.dim)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(K.S.sm)
                }
                .frame(height: 90)
                .background(K.C.well)
            }
        }
        .frame(width: 560)
        .background(K.C.bg)
        .task { if loading { await refresh() } }
        .task { if keelLoading { await refreshKeel() } }
    }

    @ViewBuilder private var keelSection: some View {
        let shown = Self.keelHits(keel, query: query)
        if keelLoading || keelFailure != nil || !shown.isEmpty {
            Text("FROM KEEL")
                .sectionLabel()
                .foregroundStyle(K.C.faint)
                .padding(.horizontal, K.S.md).padding(.top, K.S.sm)
            if keelLoading {
                HStack { ProgressView().controlSize(.small); Text("Checking this project…").font(K.F.small) }
                    .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            } else if let keelFailure {
                Text("Could not check this project: \(keelFailure)")
                    .font(K.F.micro).foregroundStyle(K.C.del)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            }
            ForEach(shown) { s in keelRow(s) }
            Hairline()
        }
    }

    private func keelRow(_ s: KeelSkill) -> some View {
        HStack(alignment: .top, spacing: K.S.sm) {
            VStack(alignment: .leading, spacing: K.S.xxs) {
                HStack(spacing: K.S.xs) {
                    Text(s.id).font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                    Text("keel").font(K.F.codeTiny).foregroundStyle(K.C.faint)
                }
                Text(s.description).font(K.F.micro).foregroundStyle(K.C.dim)
                    .lineLimit(2).fixedSize(horizontal: false, vertical: true)
                (Text(Self.facts(s)).foregroundStyle(K.C.faint)
                 + Text(s.script != nil && !s.python
                        ? " · python3 not found — its search script will not run until python3 is on PATH" : "")
                    .foregroundStyle(K.C.warn))
                    .font(K.F.micro)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: K.S.sm)
            switch s.present {
            case "project": Text("in project").font(K.F.micro).foregroundStyle(K.C.add)
            case "user": Text("in your skills").font(K.F.micro).foregroundStyle(K.C.add)
            default:
                Button(installing == "keel:" + s.id ? "…" : "Add") { add(s) }
                    .buttonStyle(QuietButton())
                    .disabled(installing != nil)
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
    }

    private func refreshKeel() async {
        keelLoading = true
        defer { keelLoading = false }
        do {
            let c: KeelCatalog = try await client.get("/api/skills/catalog")
            keel = c.entries
            keelFailure = nil
        } catch { keelFailure = error.localizedDescription }
    }

    struct AddBody: Encodable { var id: String; var commit: Bool }

    private func add(_ s: KeelSkill) {
        installing = "keel:" + s.id
        log = ""
        let commit = autoCommit
        Task {
            defer { installing = nil }
            do {
                let r: Installed = try await client.post("/api/skills/add",
                                                          body: AddBody(id: s.id, commit: commit),
                                                          as: Installed.self)
                log = Self.added(r, asked: commit)
            } catch {
                log = "Could not write .claude/skills/\(s.id): \(error.localizedDescription)"
            }
            await refreshKeel()
        }
    }

    private func refresh() async {
        loading = true
        defer { loading = false }
        do {
            let c: Catalog = try await client.get("/api/plugins")
            suggested = c.suggested
            all = c.all
            failure = nil
        } catch { failure = error.localizedDescription }
    }

    private func row(_ e: Entry) -> some View {
        HStack(alignment: .top, spacing: K.S.sm) {
            VStack(alignment: .leading, spacing: K.S.xxs) {
                HStack(spacing: K.S.xs) {
                    Text(e.name).font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                    Text(e.marketplace).font(K.F.codeTiny).foregroundStyle(K.C.faint)
                }
                // A recommendation is a row that has to argue for itself.
                if let why = e.reason, !why.isEmpty {
                    Text(why).font(K.F.micro).foregroundStyle(K.C.accent)
                        .fixedSize(horizontal: false, vertical: true)
                } else if let d = e.description, !d.isEmpty {
                    Text(d).font(K.F.micro).foregroundStyle(K.C.dim)
                        .lineLimit(2).fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: K.S.sm)
            if e.installed {
                Text("installed").font(K.F.micro).foregroundStyle(K.C.add)
            } else {
                Button(installing == e.id ? "…" : "Install") { install(e) }
                    .buttonStyle(QuietButton())
                    .disabled(installing != nil)
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
    }

    private func install(_ e: Entry) {
        installing = e.id
        log = ""
        Task {
            defer { installing = nil }
            var result = SetupCommandResult()
            do {
                for try await ev in client.events("/api/plugins/install",
                                                  ["name": e.name, "marketplace": e.marketplace]) {
                    result.receive(ev)
                    switch ev.name {
                    case "line", "fatal": log += ev.data + "\n"
                    case "done":
                        if let c: Catalog = try? await client.get("/api/plugins") {
                            suggested = c.suggested
                            all = c.all
                        }
                    default: break
                    }
                }
                if let failure = result.failure { log += failure + "\n" }
            } catch { log += error.localizedDescription }
        }
    }
}

struct AddMCP: View {
    let client: Client
    let done: () -> Void

    @State private var name = ""
    @State private var transport = "http"
    @State private var target = ""
    @State private var scope = "local"
    @State private var log = ""
    @State private var adding = false

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            Text("Add an MCP server").font(K.F.title).foregroundStyle(K.C.text)
            field("Name", "linear", $name)
            Picker("Transport", selection: $transport) {
                Text("HTTP").tag("http"); Text("SSE").tag("sse"); Text("stdio").tag("stdio")
            }
            .pickerStyle(.segmented).labelsHidden()
            field(transport == "stdio" ? "Command" : "URL",
                  transport == "stdio" ? "npx -y some-server" : "https://…", $target)
            Picker("Scope", selection: $scope) {
                Text("This machine").tag("local")
                Text("All my projects").tag("user")
            }
            .pickerStyle(.segmented).labelsHidden()

            // `project` is deliberately absent: writing a server into the repository configures a
            // command to run on the machine of whoever clones it next.
            Text("Keel never writes a server into the repository — that would be configuring a "
                 + "command to run on someone else's machine.")
                .font(K.F.micro).foregroundStyle(K.C.dim)
                .fixedSize(horizontal: false, vertical: true)

            if !log.isEmpty {
                Text(log).font(K.F.codeSmall).foregroundStyle(K.C.dim).lineLimit(6)
            }

            HStack {
                Spacer()
                Button("Cancel") { done() }.buttonStyle(QuietButton()).disabled(adding)
                Button(adding ? "Adding…" : "Add server") { add() }
                    .buttonStyle(FilledButton())
                    .disabled(adding || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || target.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(K.S.xl).frame(width: 440).background(K.C.bg)
        .interactiveDismissDisabled(adding)
    }

    private func field(_ label: String, _ hint: String, _ value: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: K.S.tight) {
            Text(label).font(K.F.micro).foregroundStyle(K.C.faint)
            TextField(hint, text: value)
                .textFieldStyle(.plain).font(K.F.code)
                .padding(.horizontal, K.S.sm).padding(.vertical, K.S.snug)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        }
    }

    private func add() {
        guard !adding else { return }
        adding = true
        log = ""
        Task {
            defer { adding = false }
            var result = SetupCommandResult()
            do {
                for try await e in client.events("/api/mcp/add", [
                    "name": name, "transport": transport, "target": target, "scope": scope,
                ]) {
                    result.receive(e)
                    switch e.name {
                    case "line", "fatal": log += e.data + "\n"
                    case "done": break
                    default: break
                    }
                }
                if result.succeeded { done() }
                else if let failure = result.failure { log += failure + "\n" }
            } catch { log += error.localizedDescription }
        }
    }
}

struct NewSubagent: View {
    let client: Client
    var autoCommit = true
    let done: () -> Void

    @State private var name = ""
    @State private var about = ""
    @State private var prompt = ""
    @State private var error: String?

    struct NewAgentBody: Encodable {
        var name: String; var description: String; var tools: String; var prompt: String; var commit: Bool
    }
    struct Created: Decodable { var path: String; var note: String? }
    @State private var created: String?

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            Text("Create a subagent").font(K.F.title).foregroundStyle(K.C.text)

            VStack(alignment: .leading, spacing: K.S.tight) {
                Text("Name").font(K.F.micro).foregroundStyle(K.C.faint)
                TextField("test-runner", text: $name)
                    .textFieldStyle(.plain).font(K.F.code)
                    .padding(.horizontal, K.S.sm).padding(.vertical, K.S.snug)
                    .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                    .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            }

            editor("When to use it",
                   "The only thing the main agent reads when deciding whether to delegate.",
                   $about, height: 60)
            editor("Its instructions", nil, $prompt, height: 100)

            if let created {
                Text(created).font(K.F.small).foregroundStyle(K.C.warn)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let error {
                Text(error).font(K.F.small).foregroundStyle(K.C.del)
            }

            HStack {
                Spacer()
                if created != nil {
                    Button("Done") { done() }.buttonStyle(FilledButton())
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("Cancel") { done() }.buttonStyle(QuietButton())
                    Button("Create") { create() }
                        .buttonStyle(FilledButton())
                        .disabled(name.isEmpty || about.isEmpty)
                }
            }
        }
        .padding(K.S.xl).frame(width: 480).background(K.C.bg)
    }

    private func editor(
        _ label: String, _ note: String?, _ value: Binding<String>, height: CGFloat
    ) -> some View {
        VStack(alignment: .leading, spacing: K.S.tight) {
            Text(label).font(K.F.micro).foregroundStyle(K.C.faint)
            TextEditor(text: value)
                .font(K.F.body).scrollContentBackground(.hidden)
                .frame(height: height)
                .padding(.horizontal, K.S.xs).padding(.vertical, K.S.tight)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            if let note {
                Text(note).font(K.F.micro).foregroundStyle(K.C.dim)
            }
        }
    }

    private func create() {
        Task {
            do {
                let r = try await client.post("/api/agents/create",
                                              body: NewAgentBody(name: name, description: about,
                                                                 tools: "", prompt: prompt, commit: autoCommit),
                                              as: Created.self)
                if let note = r.note, !note.isEmpty {
                    created = "Wrote \(r.path) — not committed: \(note)"
                } else {
                    done()
                }
            } catch { self.error = error.localizedDescription }
        }
    }
}

/// A skill the person writes: `.claude/skills/<name>/SKILL.md`, frontmatter and instructions.
/// The same shape as `NewSubagent`, because the two are the same act — writing down something
/// Claude should know — and differ only in when Claude reads it.
struct NewSkill: View {
    let client: Client
    var autoCommit = true
    let done: () -> Void

    @State private var name = ""
    @State private var about = ""
    @State private var instructions = ""
    @State private var error: String?
    @State private var creating = false
    /// What was written, when it could not also be committed — shown before the sheet closes.
    @State private var created: String?
    @FocusState private var nameFocused: Bool

    struct CreateBody: Encodable { var name: String; var description: String; var instructions: String; var commit: Bool }

    /// The daemon's rule (`agents::valid_name`), checked as the person types.
    static func validName(_ n: String) -> Bool {
        !n.isEmpty && n.count <= 64 && !n.hasPrefix("-")
            && n.allSatisfy { ("a"..."z").contains($0) || ("0"..."9").contains($0) || $0 == "-" }
    }

    private var nameError: String? {
        name.isEmpty || Self.validName(name) ? nil : "Use lowercase letters, digits and hyphens, at most 64 characters — the name becomes a folder."
    }
    private var ready: Bool {
        Self.validName(name) && !about.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !creating
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            Text("Create a skill").font(K.F.title).foregroundStyle(K.C.text)

            VStack(alignment: .leading, spacing: K.S.tight) {
                Text("Name").font(K.F.micro).foregroundStyle(K.C.faint)
                TextField("release-notes", text: $name)
                    .textFieldStyle(.plain).font(K.F.code)
                    .focused($nameFocused)
                    .padding(.horizontal, K.S.sm).padding(.vertical, K.S.snug)
                    .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                    .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
                Text("Writes .claude/skills/\(name.isEmpty ? "<name>" : name)/SKILL.md")
                    .font(K.F.micro).foregroundStyle(K.C.dim)
            }

            editor("When to use it",
                   "Claude reads only this when deciding whether to load the skill.",
                   $about, height: 60)
            editor("Instructions", nil, $instructions, height: 140)

            if let created {
                Text(created).font(K.F.small).foregroundStyle(K.C.warn)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let message = nameError ?? error {
                Text(message).font(K.F.small).foregroundStyle(K.C.del)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack {
                Spacer()
                if created != nil {
                    Button("Done") { done() }.buttonStyle(FilledButton())
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("Cancel") { done() }.buttonStyle(QuietButton())
                        .keyboardShortcut(.cancelAction)
                    Button(creating ? "…" : "Create") { create() }
                        .buttonStyle(FilledButton())
                        .keyboardShortcut(.return, modifiers: .command)
                        .disabled(!ready)
                }
            }
        }
        .padding(K.S.xl).frame(width: 480).background(K.C.bg)
        .onAppear { nameFocused = true }
    }

    private func editor(
        _ label: String, _ note: String?, _ value: Binding<String>, height: CGFloat
    ) -> some View {
        VStack(alignment: .leading, spacing: K.S.tight) {
            Text(label).font(K.F.micro).foregroundStyle(K.C.faint)
            TextEditor(text: value)
                .font(K.F.body).scrollContentBackground(.hidden)
                .frame(height: height)
                .padding(.horizontal, K.S.xs).padding(.vertical, K.S.tight)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            if let note {
                Text(note).font(K.F.micro).foregroundStyle(K.C.dim)
            }
        }
    }

    private func create() {
        guard ready else { return }
        creating = true
        error = nil
        Task {
            defer { creating = false }
            do {
                let r = try await client.post("/api/skills/create",
                                              body: CreateBody(name: name, description: about,
                                                               instructions: instructions, commit: autoCommit),
                                              as: SkillCatalog.Installed.self)
                if autoCommit, r.committed == nil || r.note != nil {
                    created = SkillCatalog.added(r, asked: true)
                } else {
                    done()
                }
            } catch {
                self.error = error.localizedDescription
                nameFocused = true
            }
        }
    }
}
