import AppKit
import SwiftUI

/// The uncommitted changes, as the tree they actually are.
///
/// A flat list of forty paths is unreadable in a 256pt rail — every row truncates to
/// `…/src/handlers/thing.rs` and you scan the same prefix forty times. Grouping by directory puts
/// the shared prefix once, at the top, where it belongs.
///
/// Directories with a single child collapse into one row (`crates/keel/src`), because a chain of
/// three folders holding one folder each is three rows saying nothing.
enum ChangeTree {
    final class Node: Identifiable {
        let name: String
        let path: String
        let isDir: Bool
        var children: [Node] = []
        var change: Wire.Change?
        /// A collapsed directory can share its path with a file sibling; the kind keeps them apart.
        var id: String { (isDir ? "d:" : "f:") + path }

        init(name: String, path: String, isDir: Bool, change: Wire.Change? = nil) {
            self.name = name
            self.path = path
            self.isDir = isDir
            self.change = change
        }

        /// Every file beneath this node, for the count on a folder row.
        var fileCount: Int {
            isDir ? children.reduce(0) { $0 + $1.fileCount } : 1
        }
    }

    static func build(_ changes: [Wire.Change]) -> [Node] {
        let root = Node(name: "", path: "", isDir: true)

        for change in changes {
            var parts = change.path.split(separator: "/").map(String.init)
            guard let file = parts.popLast() else { continue }
            var cursor = root
            var prefix = ""
            for part in parts {
                prefix = prefix.isEmpty ? part : prefix + "/" + part
                if let existing = cursor.children.first(where: { $0.isDir && $0.name == part }) {
                    cursor = existing
                } else {
                    let node = Node(name: part, path: prefix, isDir: true)
                    cursor.children.append(node)
                    cursor = node
                }
            }
            cursor.children.append(
                Node(name: file, path: change.path, isDir: false, change: change))
        }

        sort(root)
        return collapse(root.children)
    }

    /// Folders first, then files, each alphabetically — the order every file browser uses, and the
    /// one people can predict.
    private static func sort(_ node: Node) {
        node.children.sort {
            $0.isDir == $1.isDir ? $0.name.lowercased() < $1.name.lowercased() : $0.isDir
        }
        node.children.forEach(sort)
    }

    /// `crates` → `keel` → `src` becomes `crates/keel/src`.
    private static func collapse(_ nodes: [Node]) -> [Node] {
        nodes.map { node in
            guard node.isDir else { return node }
            var current = node
            var name = node.name
            while current.children.count == 1, let only = current.children.first, only.isDir {
                name += "/" + only.name
                current = only
            }
            let merged = Node(name: name, path: current.path, isDir: true)
            merged.children = collapse(current.children)
            return merged
        }
    }
}

struct ChangesTreeView: View {
    let model: SessionModel

    var body: some View {
        if !model.isRepo {
            NotARepo(model: model)
        } else {
            repoContents
        }
    }

    @ViewBuilder
    private var repoContents: some View {
        // The pull request lives in the Git panel with the rest of the branch work; a big
        // button here read as the thing to click and was the thing nobody wanted.

        if let err = model.lastError {
            Text(err).font(K.F.micro).foregroundStyle(K.C.del)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, K.S.md).padding(.bottom, K.S.sm)
        }

        // What the agent changed in this conversation, as a tree. Git's own view — staged,
        // unstaged, committed — is the Git tab's.
        let edited = model.editedThisSession
        if edited.isEmpty {
            Text(model.turns.isEmpty ? "Nothing yet. Files the agent writes in this conversation appear here."
                                     : "The agent has not written a file in this conversation.")
                .font(K.F.small).foregroundStyle(K.C.faint)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        } else {
            ForEach(ChangeTree.build(edited)) { node in
                ChangeRow(node: node, depth: 0, model: model)
            }
        }
    }
}

/// What has been committed, newest first, with the way back.
///
/// For someone who has not lived in git, "the agent changed 40 files" is alarming and "the agent
/// made 6 small commits, here they are, undo the last one if you like" is not. Same work.
struct CommitList: View {
    @Bindable var model: SessionModel
    @State private var confirmingUndo = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: K.S.xs) {
                Text("Commit history").font(K.F.small.weight(.medium))
                    .foregroundStyle(K.C.dim)
                let local = model.commits.filter { !$0.pushed }.count
                if local > 0 {
                    Button(model.pushing ? "Pushing…" : "Push \(local)") { Task { await model.push() } }
                        .buttonStyle(QuietButton(tone: K.C.accent))
                        .disabled(model.pushing)
                        .help("\(local) commit\(local == 1 ? "" : "s") only on this Mac. Send them to the remote.")
                }
                Spacer()
            }
            .padding(.horizontal, K.S.md).padding(.top, K.S.lg).padding(.bottom, K.S.sm)

            Toggle("Commit passing turns automatically",
                   isOn: Binding(get: { model.autoCommit }, set: { model.autoCommit = $0 }))
                .toggleStyle(.checkbox).controlSize(.mini)
                .font(K.F.micro).foregroundStyle(K.C.dim)
                .padding(.horizontal, K.S.md).padding(.bottom, K.S.xs)
                .help("Commit the agent's work after each turn the project's checks accept")

            if model.commits.isEmpty {
                Text("No commits yet.").font(K.F.small).foregroundStyle(K.C.faint)
                    .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            }
            ForEach(Array(model.commits.enumerated()), id: \.element.id) { i, c in
                HoverRow(selected: model.viewingCommit?.sha == c.sha) {
                    HStack(alignment: .top, spacing: K.S.sm) {
                        Image(systemName: "circle.fill").font(.system(size: 5))
                            .foregroundStyle(i == 0 ? K.C.accent : K.C.faint).padding(.top, 5)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(c.subject).font(K.F.small).foregroundStyle(K.C.text)
                                .lineLimit(2).fixedSize(horizontal: false, vertical: true)
                            HStack(spacing: K.S.xs) {
                                // Where it is: on this Mac only, or on the remote too.
                                Pill(text: c.pushed ? "PUSHED" : "LOCAL", tone: c.pushed ? .good : .warn)
                                Text(c.sha).font(K.F.mono(10))
                                Text(c.date, style: .relative).font(K.F.mono(10))
                                Text("ago").font(K.F.mono(10))
                                if c.files > 0 {
                                    Text("· \(c.files) file\(c.files == 1 ? "" : "s")").font(K.F.mono(10))
                                }
                            }
                            .foregroundStyle(K.C.faint)
                        }
                    }
                } action: {
                    model.show(commit: c)
                }
                .contextMenu {
                    if i == 0, model.commits.count > 1 {
                        Button("Undo this commit (keep the changes)") { confirmingUndo = true }
                    }
                    Button("Copy sha") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(c.sha, forType: .string)
                    }
                }
            }
            if model.commits.count > 1 {
                Button("Undo last commit, keep the changes") { confirmingUndo = true }
                    .buttonStyle(QuietButton())
                    .padding(.horizontal, K.S.md).padding(.top, K.S.xs)
                    .alert("Undo the last commit?", isPresented: $confirmingUndo) {
                        Button("Undo") { Task { await model.uncommit() } }
                        Button("Cancel", role: .cancel) {}
                    } message: {
                        Text("The commit goes away; every change in it stays in your files, "
                             + "uncommitted, so you can look again or discard pieces.")
                    }
            }
        }
    }
}

/// A project that is not under git yet.
///
/// Not an error. A new project is not a broken one, and "fatal: not a git repository" is a message
/// about a tool's expectations rather than about anything the person did wrong. So this says what
/// is missing, why it matters here, and offers the one command that fixes it.
struct NotARepo: View {
    let model: SessionModel
    @State private var running = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            Text("Not a git repository")
                .font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
            Text("Keel shows what the agent changed by diffing against git, and the project's own "
                 + "history is what makes a change reviewable. Without it, a turn can still edit "
                 + "files — there is just no before to compare them to.")
                .font(K.F.micro).foregroundStyle(K.C.dim)
                .fixedSize(horizontal: false, vertical: true)

            Button(running ? "Initialising…" : "Initialise a repository") {
                running = true
                Task {
                    error = await model.gitInit()
                    running = false
                }
            }
            .buttonStyle(SendButtonWide())
            .disabled(running)

            Text("Runs `git init` here. No first commit and no `.gitignore` — those are yours.")
                .font(.system(size: 10)).foregroundStyle(K.C.faint)

            if let error {
                Text(error).font(K.F.micro).foregroundStyle(K.C.del)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(K.S.md)
    }
}

/// Opening a pull request, through `gh`.
///
/// Keel holds no GitHub token of its own — `gh` already knows how to authenticate, find the remote
/// and pick a base — so this drives it and shows you exactly what it ran.
struct PullRequest: View {
    let model: SessionModel
    let done: () -> Void

    @State private var attempt = 0
    @State private var title = ""
    @State private var body_ = ""
    @State private var draft = false
    @State private var log = ""
    @State private var running = false
    @State private var url: String?
    @State private var failed = false

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            Text("Open a pull request").font(K.F.title).foregroundStyle(K.C.text)

            HStack(spacing: K.S.sm) {
                Image(systemName: "arrow.triangle.branch")
                    .font(.system(size: 10)).foregroundStyle(K.C.faint)
                Text(model.branch ?? "—").font(K.F.code).foregroundStyle(K.C.dim)
                if !model.changes.isEmpty {
                    Pill(text: "\(model.changes.count) UNCOMMITTED", tone: .warn)
                }
            }

            if let blocker = model.mergeBlocker {
                Text(blocker).font(K.F.small).foregroundStyle(K.C.warn)
                    .fixedSize(horizontal: false, vertical: true)
            }

            field("Title", "left empty, gh writes it from your commits", $title)
            editor("Description", $body_)
            Toggle("Open as a draft", isOn: $draft)
                .toggleStyle(.checkbox).font(K.F.small)

            if !log.isEmpty {
                ScrollView {
                    Text(log)
                        .font(K.F.codeSmall)
                        .foregroundStyle(failed ? K.C.del : K.C.dim)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(K.S.sm)
                }
                .frame(height: 130)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            }

            HStack {
                if let url {
                    Button("Open in browser") {
                        if let u = URL(string: url) { NSWorkspace.shared.open(u) }
                    }
                    .buttonStyle(QuietButton(tone: K.C.add))
                }
                Spacer()
                Button(url == nil ? "Cancel" : "Done") { done() }
                    .buttonStyle(QuietButton())
                if url == nil {
                    Button(running ? "Opening…" : "Create") { create() }
                        .buttonStyle(SendButtonWide())
                        .disabled(running || model.mergeBlocker != nil)
                }
            }
        }
        .padding(K.S.xl)
        .frame(width: 520)
        .background(K.C.bg)
        .task {
            if title.isEmpty { title = model.title }
            if body_.isEmpty {
                do { body_ = try PacketStore.markdown(model: model) }
                catch { model.lastError = error.localizedDescription }
            }
        }
        .task(id: attempt) { if attempt > 0 { await run() } }
    }

    private func field(_ label: String, _ hint: String, _ value: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(label).font(K.F.micro).foregroundStyle(K.C.faint)
            TextField(hint, text: value)
                .textFieldStyle(.plain).font(K.F.body)
                .padding(.horizontal, K.S.sm).padding(.vertical, 5)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        }
    }

    private func editor(_ label: String, _ value: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(label).font(K.F.micro).foregroundStyle(K.C.faint)
            TextEditor(text: value)
                .font(K.F.body).scrollContentBackground(.hidden)
                .frame(height: 90)
                .padding(.horizontal, K.S.xs).padding(.vertical, 3)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        }
    }

    /// Bumped by Create. The work runs in `.task(id:)`, so closing the sheet cancels it
    /// rather than leaving a stream writing into state that no longer has a view.
    private func create() {
        attempt += 1
    }

    private func run() async {
        running = true
        log = ""
        failed = false
        defer { running = false }
            do {
                for try await e in model.client.events("/api/github/pr", model.q([
                    "title": title, "body": body_, "draft": draft ? "true" : "false",
                ])) {
                    switch e.name {
                    case "line":
                        log += e.data + "\n"
                        // `gh` prints the pull request's address as its last line; that is the
                        // thing you actually wanted, so it gets a button rather than a scrollback.
                        if e.data.contains("://") , let found = e.data
                            .split(separator: " ")
                            .first(where: { $0.contains("://") }) {
                            url = String(found)
                        }
                    case "fatal":
                        log += e.data + "\n"
                        failed = true
                    case "done":
                        if e.data != "0" { failed = true }
                        Telemetry.track("pr_opened", ["ok": e.data == "0", "draft": draft])
                        await model.refreshGit()
                    default: break
                    }
                }
            } catch is CancellationError {
                // The sheet closed. Nothing to say.
            } catch {
                log += error.localizedDescription
                failed = true
            }
    }
}

struct ChangeRow: View {
    let node: ChangeTree.Node
    let depth: Int
    let model: SessionModel
    @State private var open = true

    var body: some View {
        if node.isDir {
            HoverRow {
                HStack(spacing: K.S.xs) {
                    Image(systemName: open ? "chevron.down" : "chevron.right")
                        .font(.system(size: 7, weight: .bold))
                        .foregroundStyle(K.C.faint).frame(width: 8)
                    Image(systemName: "folder")
                        .font(.system(size: 10)).foregroundStyle(K.C.faint).frame(width: 11)
                    Text(node.name)
                        .font(K.F.mono(11)).foregroundStyle(K.C.dim)
                        .lineLimit(1).truncationMode(.head)
                    Spacer(minLength: K.S.xs)
                    Text("\(node.fileCount)")
                        .font(K.F.mono(10)).foregroundStyle(K.C.faint)
                }
                .padding(.leading, CGFloat(depth) * 10)
            } action: {
                withAnimation(K.M.quick) { open.toggle() }
            }

            if open {
                ForEach(node.children) { child in
                    ChangeRow(node: child, depth: depth + 1, model: model)
                }
            }
        } else {
            HoverRow(selected: model.viewingDiff == node.path) {
                HStack(spacing: K.S.xs) {
                    Spacer().frame(width: 8)
                    Text(String((node.change?.label ?? "?").prefix(1)).uppercased())
                        .font(K.F.mono(10, .bold))
                        .foregroundStyle(tint)
                        .frame(width: 11)
                    Text(node.name)
                        .font(K.F.mono(11)).foregroundStyle(K.C.text)
                        .lineLimit(1).truncationMode(.middle)
                    Spacer()
                }
                .padding(.leading, CGFloat(depth) * 10)
            } action: {
                model.show(diff: node.path)
            }
            .help(node.path)
            .contextMenu {
                Button("Stage") { Task { await model.gitAct("stage", node.path) } }
                Button("Unstage") { Task { await model.gitAct("unstage", node.path) } }
                Button("Attach as context") { model.mention(node.path) }
                Button("Reveal in Finder") { Task { await model.reveal(node.path) } }
                // The answer to "why is my .env.local in here": git should not be watching it.
                // The file stays on disk; the index forgets it.
                Button("Add to .gitignore") { Task { await model.gitIgnore(node.path) } }
                Divider()
                Button("Discard changes", role: .destructive) {
                    Task { await model.gitAct("discard", node.path) }
                }
            }
        }
    }

    private var tint: Color {
        let s = (node.change?.status ?? "").trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("?") { return K.C.accent }
        if s.hasPrefix("D") { return K.C.del }
        if s.hasPrefix("A") { return K.C.add }
        return K.C.warn
    }
}


/// One commit, file by file, in the stage.
///
/// Clicking a commit in the panel did nothing, which is the one thing a list of commits must
/// not do. The files are the working-tree diff view with a different source.
struct CommitSurface: View {
    let model: SessionModel
    let commit: Wire.Commit
    @State private var diffs: [Wire.Diff] = []
    @State private var loading = true

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: K.S.sm) {
                Pill(text: commit.pushed ? "PUSHED" : "LOCAL", tone: commit.pushed ? .good : .warn)
                Text(commit.subject).font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                    .lineLimit(1)
                Text(commit.sha).font(K.F.mono(10)).foregroundStyle(K.C.faint)
                Spacer()
                Text("\(diffs.count) file\(diffs.count == 1 ? "" : "s")")
                    .font(K.F.mono(10)).foregroundStyle(K.C.faint)
                CloseButton(label: "Close commit") { model.viewingCommit = nil }
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            .background(K.C.surface)
            Hairline()
            if loading {
                Text("Reading…").font(K.F.small).foregroundStyle(K.C.faint)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: K.S.sm) {
                        ForEach(diffs, id: \.path) { d in
                            CommitFile(diff: d, model: model)
                        }
                    }
                    .padding(K.S.md)
                }
            }
        }
        .background(K.C.bg)
        .task(id: commit.sha) {
            loading = true
            diffs = await model.commitDiff(commit.sha)
            loading = false
        }
    }
}

private struct CommitFile: View {
    let diff: Wire.Diff
    let model: SessionModel
    @State private var open = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: K.S.sm) {
                Image(systemName: open ? "chevron.down" : "chevron.right")
                    .font(.system(size: 10, weight: .bold)).foregroundStyle(K.C.faint).frame(width: 10)
                Text(diff.path).font(K.F.mono(11.5, .medium)).foregroundStyle(K.C.text)
                    .lineLimit(1).truncationMode(.head)
                Spacer()
                let adds = diff.hunks.flatMap(\.lines).count { $0.kind == "add" }
                let dels = diff.hunks.flatMap(\.lines).count { $0.kind == "del" }
                DiffBar(adds: adds, dels: dels)
                Text("+\(adds)").font(K.F.mono(10)).foregroundStyle(K.C.add)
                Text("−\(dels)").font(K.F.mono(10)).foregroundStyle(K.C.del)
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            .contentShape(Rectangle())
            .asButton { withAnimation(K.M.quick) { open.toggle() } }
            if open {
                Hairline()
                ForEach(Array(diff.hunks.enumerated()), id: \.offset) { _, hunk in
                    Text(hunk.header).font(K.F.mono(10)).foregroundStyle(K.C.faint)
                        .padding(.horizontal, K.S.md).padding(.vertical, 3)
                        .frame(maxWidth: .infinity, alignment: .leading).background(K.C.well)
                    let marks = Intraline.marks(hunk.lines)
                    ForEach(Array(hunk.lines.enumerated()), id: \.offset) { li, line in
                        DiffLineRow(line: line, mark: marks[li], path: diff.path, model: model)
                    }
                }
            }
        }
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
    }
}
