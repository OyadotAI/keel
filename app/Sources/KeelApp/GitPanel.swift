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
    @State private var allBranches = false
    @State private var allRemote = false

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
            HStack(spacing: K.S.xs) {
                verb("Fetch", "arrow.triangle.2.circlepath", "fetch", enabled: !(b?.remotes.isEmpty ?? true))
                verb("Pull", "arrow.down", "pull", enabled: (current?.behind ?? 0) > 0)
                verb("Push", "arrow.up", "push",
                     enabled: (current?.ahead ?? 0) > 0 || current?.upstream == nil && !(b?.remotes.isEmpty ?? true))
                if let what = model.gitBusy {
                    Sweep().scaleEffect(0.7, anchor: .leading).frame(width: 32, height: 3)
                    Text(what + "…").font(K.F.micro).foregroundStyle(K.C.faint)
                }
                Spacer()
                if let c = current, let up = c.upstream {
                    Text(up).font(K.F.mono(10)).foregroundStyle(K.C.faint).lineLimit(1)
                        .truncationMode(.head).help("Tracks \(up)")
                }
            }
            if let err = model.lastError {
                Text(err).font(K.F.micro).foregroundStyle(K.C.del)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
    }

    /// An icon, a tooltip, and a count when there is one. The word is in the tooltip.
    private func verb(_ title: String, _ icon: String, _ action: String, enabled: Bool) -> some View {
        let n = action == "pull" ? (current?.behind ?? 0) : action == "push" ? (current?.ahead ?? 0) : 0
        return Button { Task { await model.remote(action) } } label: {
            HStack(spacing: 3) {
                Image(systemName: icon).font(.system(size: 10, weight: .semibold))
                if n > 0 { Text("\(n)").font(K.F.mono(10)) }
            }
            .frame(minWidth: 22, minHeight: 16)
        }
        .buttonStyle(QuietButton(tone: enabled ? K.C.accent : K.C.faint))
        .disabled(busy || !enabled)
        .hint(n > 0 ? "\(title) \(n)" : title)
    }

    // MARK: Commit

    /// One field, one button. The button commits what is staged when something is, and
    /// everything when nothing is — which is what the person meant in both cases.
    private var commitBox: some View {
        let staged = b?.staged ?? 0, unstaged = b?.unstaged ?? 0
        let all = staged == 0
        let n = all ? model.changes.count : staged
        return VStack(alignment: .leading, spacing: K.S.xs) {
            RailHeader("Commit", trailing: nil)
            HStack(spacing: K.S.xs) {
                TextField("What changed, in one line", text: $message)
                    .field().font(K.F.small)
                    .onSubmit { commit(all: all) }
                Button(n == 0 ? "Commit" : "Commit \(n)") { commit(all: all) }
                    .buttonStyle(QuietButton(tone: K.C.accent))
                    .disabled(busy || message.trimmingCharacters(in: .whitespaces).isEmpty || n == 0)
                    .help(all ? "Commits every change" : "Commits the \(staged) staged file\(staged == 1 ? "" : "s")")
            }
            .padding(.horizontal, K.S.md)
            HStack(spacing: K.S.xs) {
                Text(n == 0 ? "nothing to commit"
                     : (all ? "\(unstaged) unstaged — all will be committed" : "\(staged) staged · \(unstaged) unstaged"))
                    .font(K.F.micro).foregroundStyle(K.C.faint)
                Spacer()
                if unstaged > 0 {
                    Button("stage all") { Task { await model.stageAll(true) } }
                        .buttonStyle(.plain).font(K.F.micro).foregroundStyle(K.C.accent).disabled(busy)
                }
                if staged > 0 {
                    Button("unstage all") { Task { await model.stageAll(false) } }
                        .buttonStyle(.plain).font(K.F.micro).foregroundStyle(K.C.accent).disabled(busy)
                }
            }
            .padding(.horizontal, K.S.md)
        }
        .padding(.bottom, K.S.sm)
    }

    private func commit(all: Bool) {
        Task { await model.commit(message, all: all); if model.lastError == nil { message = "" } }
    }

    // MARK: Branches

    private var branches: some View {
        VStack(alignment: .leading, spacing: 0) {
            RailHeader("Branches", trailing: b.map { "\($0.local.count)" })
            // The current one first, then the rest; the last commit's subject is the tooltip.
            let local = (b?.local ?? []).sorted { ($0.current ? 0 : 1, $0.name) < ($1.current ? 0 : 1, $1.name) }
            ForEach(allBranches ? local : Array(local.prefix(6))) { br in
                HoverRow(selected: br.current) {
                    HStack(spacing: K.S.sm) {
                        Image(systemName: br.current ? "checkmark" : "arrow.triangle.branch")
                            .font(.system(size: 10, weight: br.current ? .bold : .regular))
                            .foregroundStyle(br.current ? K.C.accent : K.C.faint)
                            .frame(width: 12)
                        Text(br.name).font(K.F.small.weight(br.current ? .semibold : .regular))
                            .foregroundStyle(K.C.text).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        if br.ahead > 0 { Text("\(br.ahead)↑").font(K.F.mono(10)).foregroundStyle(K.C.warn) }
                        if br.behind > 0 { Text("\(br.behind)↓").font(K.F.mono(10)).foregroundStyle(K.C.accent) }
                    }
                    .help(br.subject + (br.upstream.map { " — tracks \($0)" } ?? " — not pushed"))
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

            if local.count > 6 {
                Button(allBranches ? "Show fewer" : "Show all \(local.count)") { allBranches.toggle() }
                    .buttonStyle(.plain).font(K.F.micro).foregroundStyle(K.C.accent)
                    .padding(.horizontal, K.S.md).padding(.vertical, K.S.xs)
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

            // Branches only the remote has, without the remote's name repeated on every row
            // and without the remote's own HEAD entry.
            let remote = (b?.remote ?? []).filter { $0.contains("/") }
            if !remote.isEmpty {
                RailHeader("On \(b?.remotes.first ?? "the remote") only", trailing: "\(remote.count)")
                ForEach(allRemote ? remote : Array(remote.prefix(5)), id: \.self) { name in
                    HoverRow {
                        HStack(spacing: K.S.sm) {
                            Image(systemName: "icloud").font(.system(size: 10)).foregroundStyle(K.C.faint)
                                .frame(width: 12)
                            Text(name.split(separator: "/", maxSplits: 1).last.map(String.init) ?? name)
                                .font(K.F.small).foregroundStyle(K.C.dim).lineLimit(1).truncationMode(.middle)
                            Spacer()
                        }
                        .help("Check out \(name) as a local branch")
                    } action: {
                        Task { await model.branch("checkout", name) }
                    }
                    .disabled(busy)
                }
                if remote.count > 5 {
                    Button(allRemote ? "Show fewer" : "Show all \(remote.count)") { allRemote.toggle() }
                        .buttonStyle(.plain).font(K.F.micro).foregroundStyle(K.C.accent)
                        .padding(.horizontal, K.S.md).padding(.vertical, K.S.xs)
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
