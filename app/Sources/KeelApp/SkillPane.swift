import AppKit
import SwiftUI

/// One skill, whole: every file in its folder, where it lives and where it can go, and a sentence
/// box that has the person's own `claude` change it — shown as a diff and written only on Save.
///
/// It replaced the generic inspector for skills, which showed the description the row already
/// showed and a path. A skill is a folder — `SKILL.md`, a schema, scripts, assets — and a view
/// that shows one line of it cannot answer "what will this actually do".
struct SkillPane: View {
    let model: SessionModel
    let skill: Wire.Named

    struct Shown: Decodable, Hashable { var path: String; var content: String?; var bytes: Int }
    struct Folder: Decodable { var files: [Shown]; var truncated: Bool }
    struct Proposal: Decodable { var name: String; var description: String; var files: [NewSkill.File]; var check: String }
    private struct Revise: Encodable { var dir: String; var ask: String }
    private struct Save: Encodable { var dir: String; var files: [NewSkill.File]; var commit: Bool }

    @State private var folder: Folder?
    @State private var loadError: String?
    @State private var selected = "SKILL.md"
    @State private var rendered = true
    @State private var ask = ""
    @State private var revising: Task<Void, Never>?
    @State private var started = Date()
    @State private var proposal: Proposal?
    @State private var busy = false
    @State private var message: (text: String, bad: Bool)?
    @FocusState private var askFocused: Bool

    private var dir: String { (skill.path.map { ($0 as NSString).deletingLastPathComponent }) ?? "" }
    private var origin: SkillsPanel.Origin { .of(skill) }
    private var editable: Bool { origin != .plugins }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Hairline()
            HStack(spacing: 0) {
                fileList.frame(width: 200)
                Rectangle().fill(K.C.line).frame(width: 1)
                viewer
            }
            Hairline()
            editor
        }
        .background(K.C.bg)
        .task(id: skill.id) { await load() }
    }

    // MARK: Header

    private var header: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "sparkles").font(K.F.small).foregroundStyle(K.C.accent)
                Text(skill.name).font(K.F.title).foregroundStyle(K.C.text).textSelection(.enabled)
                Pill(text: badge, tone: origin == .generated ? .accent : skill.fromRepo ? .warn : .neutral)
                Spacer()
                moveMenu
                if let path = skill.path {
                    Button { NSWorkspace.shared.selectFile(path, inFileViewerRootedAtPath: "") } label: {
                        Image(systemName: "folder")
                    }
                    .buttonStyle(QuietButton()).hint("Reveal in Finder")
                }
                if editable {
                    Button { trash() } label: { Image(systemName: "trash") }
                        .buttonStyle(QuietButton(tone: K.C.del))
                        .hint(skill.fromRepo
                              ? "Move to the Trash, and commit the removal when auto-commit is on"
                              : "Move to the Trash — it stops loading in every project")
                        .disabled(busy)
                }
                CloseButton { model.inspecting = nil }
            }
            Text(skill.description.isEmpty
                 ? "No description — Claude decides whether to load a skill by reading it, so this one will rarely be used."
                 : skill.description)
                .font(K.F.small).foregroundStyle(skill.description.isEmpty ? K.C.warn : K.C.dim)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
            Text(Inspector.skillScope(skill)).font(K.F.micro).foregroundStyle(K.C.faint)
            if let message {
                Text(message.text).font(K.F.small)
                    .foregroundStyle(message.bad ? K.C.del : K.C.warn)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
        .padding(.horizontal, K.S.lg).padding(.vertical, K.S.md)
        .background(K.C.surface)
    }

    private var badge: String {
        switch origin {
        case .project: "PROJECT"
        case .generated: "GENERATED"
        case .personal: "PERSONAL"
        case .plugins: "PLUGIN · \((skill.plugin ?? "").uppercased())"
        }
    }

    /// Where it can go from here — the tabs, as destinations.
    private var moveMenu: some View {
        Menu {
            ForEach(Self.moves(for: skill), id: \.label) { m in
                Button(m.label) { Task { await relocate(m) } }
            }
        } label: {
            Label("Move", systemImage: "arrow.left.arrow.right")
        }
        .menuStyle(.borderlessButton).fixedSize()
        .disabled(busy)
    }

    // MARK: Files

    private var fileList: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: K.S.hair) {
                ForEach(listed, id: \.self) { path in
                    let changed = proposal?.files.contains { $0.path == path } == true
                    Button { selected = path } label: {
                        HStack(spacing: K.S.xs) {
                            Image(systemName: Self.icon(path)).font(K.F.tiny).frame(width: 12)
                            Text(path).font(K.F.codeSmall).lineLimit(1).truncationMode(.middle)
                            Spacer(minLength: 0)
                            if changed { Circle().fill(K.C.accent).frame(width: 6, height: 6) }
                        }
                        .foregroundStyle(selected == path ? K.C.text : K.C.dim)
                        .padding(.horizontal, K.S.sm).padding(.vertical, K.S.snug)
                        .background(selected == path ? K.C.chrome : .clear,
                                    in: RoundedRectangle(cornerRadius: K.R.sm))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
                if folder?.truncated == true {
                    Text("Only the first 200 files are shown.").font(K.F.micro).foregroundStyle(K.C.faint)
                        .padding(K.S.sm)
                }
            }
            .padding(K.S.xs)
        }
        .background(K.C.surface)
    }

    /// The folder's files plus any the proposal adds.
    private var listed: [String] {
        var paths = folder?.files.map(\.path) ?? []
        for f in proposal?.files ?? [] where !paths.contains(f.path) { paths.append(f.path) }
        return paths
    }

    static func icon(_ path: String) -> String {
        if path == "SKILL.md" { return "doc.richtext" }
        if path.hasSuffix(".json") { return "curlybraces" }
        if path.hasSuffix(".py") || path.hasSuffix(".sh") || path.hasSuffix(".js") || path.hasSuffix(".ts") {
            return "chevron.left.forwardslash.chevron.right"
        }
        if path.hasSuffix(".md") { return "doc.text" }
        return "doc"
    }

    // MARK: Viewer

    @ViewBuilder private var viewer: some View {
        let current = folder?.files.first { $0.path == selected }
        let proposed = proposal?.files.first { $0.path == selected }
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(selected).font(K.F.codeSmall).foregroundStyle(K.C.dim)
                if let current { Text(Self.size(current.bytes)).font(K.F.codeTiny).foregroundStyle(K.C.faint) }
                Spacer()
                if selected.hasSuffix(".md"), proposed == nil, current?.content != nil {
                    Picker("", selection: $rendered) {
                        Text("Rendered").tag(true)
                        Text("Source").tag(false)
                    }
                    .pickerStyle(.segmented).labelsHidden().fixedSize()
                }
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.xs)
            Hairline()
            if let loadError {
                EmptyState(icon: "exclamationmark.triangle", title: "Could not read this skill", loadError)
            } else if folder == nil {
                ProgressView().controlSize(.small).frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let proposed {
                DiffLines(old: current?.content ?? "", new: proposed.content)
            } else if let current, let text = current.content {
                if rendered && selected.hasSuffix(".md") {
                    ScrollView {
                        Markdown(Self.body(of: text))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(K.S.lg)
                    }
                } else {
                    CodeText(text: text)
                }
            } else if let current {
                EmptyState(icon: "doc", title: "Not shown",
                           "\(Self.size(current.bytes)) — binary, or larger than 256 KB.")
            } else {
                Spacer()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// A `SKILL.md` without its frontmatter: the header above already shows the name and the
    /// description, and YAML rendered as prose reads as a broken paragraph.
    static func body(of md: String) -> String {
        guard md.hasPrefix("---\n"), let end = md.range(of: "\n---", range: md.index(md.startIndex, offsetBy: 4)..<md.endIndex)
        else { return md }
        return String(md[end.upperBound...]).trimmingCharacters(in: .newlines)
    }

    static func size(_ bytes: Int) -> String {
        bytes.formatted(.byteCount(style: .file))
    }

    // MARK: AI editor

    private var editor: some View {
        HStack(alignment: .center, spacing: K.S.sm) {
            if let proposal {
                Image(systemName: "sparkles").foregroundStyle(K.C.accent)
                VStack(alignment: .leading, spacing: K.S.hair) {
                    Text("Claude changed \(proposal.files.count) file\(proposal.files.count == 1 ? "" : "s") — review the diff, then save.")
                        .font(K.F.small).foregroundStyle(K.C.text)
                    Text(proposal.check).font(K.F.micro).foregroundStyle(K.C.faint)
                }
                Spacer()
                Button("Discard") { self.proposal = nil }.buttonStyle(QuietButton())
                    .keyboardShortcut(.cancelAction)
                Button(busy ? "…" : "Save") { save(proposal) }.buttonStyle(FilledButton())
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(busy)
            } else if !editable {
                Image(systemName: "lock").foregroundStyle(K.C.faint)
                Text("A plugin's skills are replaced when the plugin updates. Copy it to Project or Personal to edit it.")
                    .font(K.F.small).foregroundStyle(K.C.dim)
                Spacer()
            } else {
                Image(systemName: "sparkles").foregroundStyle(revising == nil ? K.C.faint : K.C.accent)
                TextField("Describe a change — “add a --json flag”, “support EUR”, “tighten the description”", text: $ask)
                    .textFieldStyle(.plain).font(K.F.body)
                    .focused($askFocused)
                    .onSubmit { revise() }
                    .disabled(revising != nil)
                if revising != nil {
                    TimelineView(.periodic(from: started, by: 1)) { ctx in
                        Text("Claude is editing… \(Int(ctx.date.timeIntervalSince(started)))s")
                            .font(K.F.small).foregroundStyle(K.C.dim)
                    }
                    Button("Stop") { revising?.cancel(); revising = nil }.buttonStyle(QuietButton())
                } else {
                    Button("Edit with Claude") { revise() }
                        .buttonStyle(FilledButton())
                        .disabled(ask.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        .padding(.horizontal, K.S.lg).padding(.vertical, K.S.md)
        .background(K.C.surface)
    }

    // MARK: Actions

    private func load() async {
        folder = nil
        loadError = nil
        proposal = nil
        message = nil
        do {
            let f: Folder = try await model.client.get("/api/skills/files", ["dir": dir])
            folder = f
            if !f.files.contains(where: { $0.path == selected }) { selected = f.files.first?.path ?? "SKILL.md" }
        } catch {
            loadError = error.localizedDescription
        }
    }

    private func revise() {
        let text = ask.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty, revising == nil else { return }
        message = nil
        started = Date()
        revising = Task {
            defer { revising = nil }
            do {
                let p = try await model.client.post("/api/skills/revise", body: Revise(dir: dir, ask: text),
                                                    as: Proposal.self, timeout: 540)
                guard !Task.isCancelled else { return }
                proposal = p
                ask = ""
                if let first = p.files.first { selected = first.path }
            } catch {
                guard !Task.isCancelled else { return }
                message = (error.localizedDescription, true)
            }
        }
    }

    private func save(_ p: Proposal) {
        busy = true
        Task {
            defer { busy = false }
            do {
                let r = try await model.client.post("/api/skills/save",
                                                    body: Save(dir: dir, files: p.files, commit: model.autoCommit),
                                                    as: SkillCatalog.Installed.self)
                proposal = nil
                if let note = r.note { message = ("Saved — not committed: \(note)", false) }
                await model.refreshState()
                await load()
            } catch {
                message = (error.localizedDescription, true)
            }
        }
    }

    private func trash() {
        busy = true
        Task {
            defer { busy = false }
            if let problem = await Self.remove(model: model, skill: skill) {
                message = (problem, true)
            } else {
                model.inspecting = nil
            }
        }
    }

    private func relocate(_ m: Move) async {
        busy = true
        defer { busy = false }
        switch await Self.move(model: model, skill: skill, m) {
        case .failure(let e): message = (e.localizedDescription, true)
        case .success(let note): if let note { message = (note, false) }
        }
    }

    // MARK: Shared with the panel's context menu

    struct Move { var label: String; var to: String; var copy: Bool }

    /// The tabs a skill can go to from its own.
    static func moves(for s: Wire.Named) -> [Move] {
        switch SkillsPanel.Origin.of(s) {
        case .project: [Move(label: "Move to Personal", to: "user", copy: false)]
        case .generated: [Move(label: "Keep in Project", to: "project", copy: false),
                          Move(label: "Move to Personal", to: "user", copy: false)]
        case .personal: [Move(label: "Move to Project", to: "project", copy: false)]
        case .plugins: [Move(label: "Copy to Project", to: "project", copy: true),
                        Move(label: "Copy to Personal", to: "user", copy: true)]
        }
    }

    private struct MoveBody: Encodable { var dir: String; var to: String; var copy: Bool; var commit: Bool }
    private struct Moved: Decodable { var dir: String; var committed: String?; var note: String? }

    /// Move it, then point the pane at where it landed. The note, when there is one, is why the
    /// commit did not happen.
    static func move(model: SessionModel, skill: Wire.Named, _ m: Move) async -> Result<String?, Error> {
        let dir = (skill.path.map { ($0 as NSString).deletingLastPathComponent }) ?? ""
        do {
            let r = try await model.client.post("/api/skills/move",
                                                 body: MoveBody(dir: dir, to: m.to, copy: m.copy, commit: model.autoCommit),
                                                 as: Moved.self)
            await model.refreshState()
            if case .skill = model.inspecting,
               let landed = model.workspace.skills.first(where: {
                   ($0.path.map { ($0 as NSString).deletingLastPathComponent }) == r.dir
               }) {
                model.inspecting = .skill(landed)
            }
            return .success(r.note.map { "Moved — not committed: \($0)" })
        } catch {
            return .failure(error)
        }
    }

    private struct RemoveBody: Encodable { var dir: String; var commit: Bool }
    private struct Removed: Decodable { var committed: String?; var note: String? }

    /// Nil when it went; otherwise what to say.
    static func remove(model: SessionModel, skill: Wire.Named) async -> String? {
        let dir = (skill.path.map { ($0 as NSString).deletingLastPathComponent }) ?? ""
        do {
            let r = try await model.client.post("/api/skills/remove",
                                                 body: RemoveBody(dir: dir, commit: model.autoCommit),
                                                 as: Removed.self)
            await model.refreshState()
            return r.note.map { "Moved to the Trash — not committed: \($0)" }
        } catch {
            return error.localizedDescription
        }
    }
}

/// A file as text, monospaced, selectable, one lazy row per line so a long script scrolls.
private struct CodeText: View {
    let text: String
    var body: some View {
        let lines = text.components(separatedBy: "\n")
        ScrollView([.vertical, .horizontal]) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(lines.indices, id: \.self) { i in
                    HStack(alignment: .top, spacing: K.S.sm) {
                        Text("\(i + 1)").font(K.F.codeTiny).foregroundStyle(K.C.faint)
                            .frame(width: 32, alignment: .trailing)
                        Text(lines[i].isEmpty ? " " : lines[i]).font(K.F.codeSmall).foregroundStyle(K.C.text)
                            .fixedSize()
                    }
                }
            }
            .textSelection(.enabled)
            .padding(K.S.md)
        }
    }
}

/// Old against new, line by line: what Claude is proposing, in the form a person who reads diffs
/// all day reads fastest.
struct DiffLines: View {
    let old: String
    let new: String

    var body: some View {
        let rows = Self.rows(old, new)
        ScrollView([.vertical, .horizontal]) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(rows.indices, id: \.self) { i in
                    let (mark, line) = rows[i]
                    HStack(spacing: K.S.sm) {
                        Text(String(mark)).font(K.F.codeSmall).frame(width: 12)
                            .foregroundStyle(mark == "+" ? K.C.add : mark == "-" ? K.C.del : K.C.faint)
                        Text(line.isEmpty ? " " : line).font(K.F.codeSmall).foregroundStyle(K.C.text).fixedSize()
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, K.S.sm)
                    .background(mark == "+" ? K.C.addBG : mark == "-" ? K.C.delBG : .clear)
                }
            }
            .textSelection(.enabled)
            .padding(.vertical, K.S.sm)
        }
    }

    /// Removed lines, then inserted ones, in file order. Past a few thousand lines it shows the
    /// new file as added rather than spend the frame on the difference.
    static func rows(_ old: String, _ new: String) -> [(Character, String)] {
        let a = old.isEmpty ? [] : old.components(separatedBy: "\n")
        let b = new.components(separatedBy: "\n")
        guard a.count + b.count < 6000 else { return b.map { ("+", $0) } }
        var removed = Set<Int>(), inserted = Set<Int>()
        for change in b.difference(from: a) {
            switch change {
            case .remove(let offset, _, _): removed.insert(offset)
            case .insert(let offset, _, _): inserted.insert(offset)
            }
        }
        var out: [(Character, String)] = []
        var i = 0, j = 0
        while i < a.count || j < b.count {
            if i < a.count, removed.contains(i) { out.append(("-", a[i])); i += 1 }
            else if j < b.count, inserted.contains(j) { out.append(("+", b[j])); j += 1 }
            else if i < a.count, j < b.count { out.append((" ", b[j])); i += 1; j += 1 }
            else { break }
        }
        return out
    }
}
