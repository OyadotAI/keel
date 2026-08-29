import SwiftUI

/// A git client, as a panel: where you are, how far from the remote, what to commit, which
/// branches exist, and the commits so far.
///
/// For a team that has not lived in git, the verbs are named for what they do and the ones that
/// can lose work are the ones the daemon refuses: `switch` will not drop edits, `pull` is
/// fast-forward only, delete is `-d`. Nothing here needs a terminal, and nothing here does what
/// a terminal would ask twice about.
struct GitPanel: View {
    @Bindable var model: SessionModel
    @State private var message = ""
    @State private var newBranch = ""
    @State private var creating = false

    private var b: Wire.Branches? { model.branches }
    private var busy: Bool { model.gitBusy != nil }

    var body: some View {
        Group {
            if !model.isRepo {
                NotARepo(model: model)
            } else {
                branchHeader
                commitBox
                branches
                CommitList(model: model)
            }
        }
        .task { await model.refreshBranches() }
        .onChange(of: model.commits.count) { Task { await model.refreshBranches() } }
    }

    // MARK: Where you are

    private var current: Wire.Branch? { b?.local.first { $0.current } }

    private var branchHeader: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "arrow.triangle.branch").font(.system(size: 11))
                    .foregroundStyle(K.C.accent)
                Text(b?.current ?? model.branch ?? "—")
                    .font(K.F.body.weight(.semibold)).foregroundStyle(K.C.text).lineLimit(1)
                Spacer()
                if let c = current, c.upstream != nil {
                    if c.ahead > 0 { Pill(text: "\(c.ahead) ↑", tone: .warn) }
                    if c.behind > 0 { Pill(text: "\(c.behind) ↓", tone: .accent) }
                    if c.ahead == 0 && c.behind == 0 { Pill(text: "IN SYNC", tone: .good) }
                } else if b != nil {
                    Pill(text: "NO REMOTE", tone: .neutral)
                }
            }
            if let c = current, let up = c.upstream {
                Text("tracks \(up)").font(K.F.mono(10)).foregroundStyle(K.C.faint)
            }
            HStack(spacing: K.S.xs) {
                verb("Fetch", "arrow.down.circle", "fetch", enabled: !(b?.remotes.isEmpty ?? true))
                verb("Pull", "arrow.down.to.line", "pull",
                     enabled: (current?.behind ?? 0) > 0)
                verb("Push", "arrow.up.to.line", "push",
                     enabled: (current?.ahead ?? 0) > 0 || current?.upstream == nil && !(b?.remotes.isEmpty ?? true))
                Spacer()
                if let what = model.gitBusy {
                    Sweep().scaleEffect(0.7, anchor: .trailing).frame(width: 32, height: 3)
                    Text(what + "…").font(K.F.micro).foregroundStyle(K.C.faint)
                }
            }
            if let err = model.lastError {
                Text(err).font(K.F.micro).foregroundStyle(K.C.del)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
    }

    private func verb(_ title: String, _ icon: String, _ action: String, enabled: Bool) -> some View {
        Button { Task { await model.remote(action) } } label: {
            Label(title, systemImage: icon).labelStyle(.titleAndIcon)
        }
        .buttonStyle(QuietButton(tone: enabled ? K.C.accent : K.C.dim))
        .disabled(busy || !enabled)
    }

    // MARK: Commit

    private var commitBox: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            RailHeader("Commit", trailing: b.map { "\($0.staged) staged · \($0.unstaged) unstaged" })
            TextField("What changed, in one line", text: $message)
                .field().font(K.F.small)
                .padding(.horizontal, K.S.md)
            HStack(spacing: K.S.xs) {
                Button("Stage all") { Task { await model.stageAll(true) } }
                    .buttonStyle(QuietButton()).disabled(busy || (b?.unstaged ?? 0) == 0)
                Button("Unstage all") { Task { await model.stageAll(false) } }
                    .buttonStyle(QuietButton()).disabled(busy || (b?.staged ?? 0) == 0)
                Spacer()
                Button("Commit staged") {
                    Task { await model.commit(message, all: false); if model.lastError == nil { message = "" } }
                }
                .buttonStyle(QuietButton(tone: K.C.accent))
                .disabled(busy || message.trimmingCharacters(in: .whitespaces).isEmpty || (b?.staged ?? 0) == 0)
                Button("Commit all") {
                    Task { await model.commit(message, all: true); if model.lastError == nil { message = "" } }
                }
                .buttonStyle(QuietButton(tone: K.C.accent))
                .disabled(busy || message.trimmingCharacters(in: .whitespaces).isEmpty || model.changes.isEmpty)
            }
            .padding(.horizontal, K.S.md)
            Text("Stage and discard individual files in Changes.")
                .font(K.F.micro).foregroundStyle(K.C.faint).padding(.horizontal, K.S.md)
        }
        .padding(.bottom, K.S.sm)
    }

    // MARK: Branches

    private var branches: some View {
        VStack(alignment: .leading, spacing: 0) {
            RailHeader("Branches", trailing: b.map { "\($0.local.count)" })
            ForEach(b?.local ?? []) { br in
                HoverRow(selected: br.current) {
                    HStack(spacing: K.S.sm) {
                        Image(systemName: br.current ? "checkmark.circle.fill" : "circle")
                            .font(.system(size: 10))
                            .foregroundStyle(br.current ? K.C.accent : K.C.faint)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(br.name).font(K.F.small.weight(br.current ? .semibold : .regular))
                                .foregroundStyle(K.C.text).lineLimit(1)
                            Text(br.subject).font(K.F.micro).foregroundStyle(K.C.faint).lineLimit(1)
                        }
                        Spacer()
                        if br.ahead > 0 { Text("\(br.ahead)↑").font(K.F.mono(10)).foregroundStyle(K.C.warn) }
                        if br.behind > 0 { Text("\(br.behind)↓").font(K.F.mono(10)).foregroundStyle(K.C.accent) }
                        if br.upstream == nil { Text("local").font(K.F.mono(10)).foregroundStyle(K.C.faint) }
                    }
                } action: {
                    if !br.current { Task { await model.branch("checkout", br.name) } }
                }
                .contextMenu {
                    if !br.current {
                        Button("Switch to \(br.name)") { Task { await model.branch("checkout", br.name) } }
                        Button("Delete \(br.name) (only if merged)", role: .destructive) {
                            Task { await model.branch("delete", br.name) }
                        }
                    }
                }
                .disabled(busy)
            }

            if creating {
                HStack(spacing: K.S.xs) {
                    TextField("new-branch-name", text: $newBranch)
                        .field().font(K.F.code)
                        .onSubmit { create() }
                    Button("Create") { create() }.buttonStyle(QuietButton(tone: K.C.accent))
                        .disabled(newBranch.trimmingCharacters(in: .whitespaces).isEmpty)
                    Button("Cancel") { creating = false; newBranch = "" }.buttonStyle(QuietButton())
                }
                .padding(.horizontal, K.S.md).padding(.vertical, K.S.xs)
            } else {
                PanelAction("New branch from here…", icon: "plus.circle") { creating = true }
            }

            if let remote = b?.remote, !remote.isEmpty {
                RailHeader("Remote only", trailing: "\(remote.count)")
                ForEach(remote, id: \.self) { name in
                    HoverRow {
                        HStack(spacing: K.S.sm) {
                            Image(systemName: "cloud").font(.system(size: 10)).foregroundStyle(K.C.faint)
                            Text(name).font(K.F.small).foregroundStyle(K.C.dim).lineLimit(1)
                            Spacer()
                            Text("check out").font(K.F.micro).foregroundStyle(K.C.faint)
                        }
                    } action: {
                        Task { await model.branch("checkout", name) }
                    }
                    .disabled(busy)
                }
            }
        }
    }

    private func create() {
        let name = newBranch.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return }
        Task {
            await model.branch("create", name)
            if model.lastError == nil { creating = false; newBranch = "" }
        }
    }
}
