import SwiftUI

/// ⌘K. Everything the app can do, reachable by typing.
///
/// The claim this makes against a terminal is that someone who came from one never has to reach
/// for the mouse. A palette is what makes that true without a menu bar item for every action, and
/// it is the single most-missed thing when a GUI replaces a CLI.
struct Palette: View {
    @Bindable var model: SessionModel
    @Binding var open: Bool
    @State private var query = ""
    @State private var selection = 0
    @FocusState private var focused: Bool

    struct Item: Identifiable {
        let id = UUID()
        let title: String
        let detail: String
        let run: () -> Void
    }

    private var items: [Item] {
        var out: [Item] = [
            Item(title: "New session", detail: "another window on this project") {
                NotificationCenter.default.post(name: .keelNewLane, object: nil)
            },
            Item(title: model.mode == "plan" ? "Switch to Auto" : "Switch to Plan",
                 detail: "what the agent is allowed to do") {
                model.mode = model.mode == "plan" ? "acceptEdits" : "plan"
            },
            Item(title: "Stop the turn", detail: "sends SIGINT, not SIGTERM") { model.stop() },
            Item(title: "Open project…", detail: "switches every lane") {
                NotificationCenter.default.post(name: .keelOpenProject, object: nil)
            },
            Item(title: "New session", detail: "another agent, running alongside") {
                NotificationCenter.default.post(name: .keelNewLane, object: nil)
            },
            Item(title: "Run the project's checks", detail: "the gate, on demand") {
                Task { await model.runGateNow() }
            },
        ]
        if !model.notes.isEmpty {
            out.append(Item(title: "Send \(model.notes.count) review comment\(model.notes.count == 1 ? "" : "s")",
                            detail: "as the next prompt") {
                model.prompt = model.commentsPrompt()
                model.notes.removeAll()
            })
        }
        for p in model.pending {
            out.append(Item(title: "Approve: \(p.command.isEmpty ? p.tool : p.command)",
                            detail: "once, this session") {
                model.answer(p, allow: true, scope: "session")
            })
        }
        // Recent projects, before files: switching project is a verb, and there are only ever a
        // handful of them.
        for path in Recents.paths.filter({ $0 != model.repoPath }) {
            out.append(Item(title: (path as NSString).lastPathComponent,
                            detail: "open project") {
                Task {
                    try? await model.openProject(path)
                    Recents.remember(path)
                    await model.lanes?.refreshShared()
                }
            })
        }

        // Files last: there are thousands, and they should not push the verbs off the list.
        for f in model.files.prefix(400) {
            out.append(Item(title: f, detail: "attach as context") { model.mention(f) })
        }
        return out
    }

    private var hits: [Item] {
        guard !query.isEmpty else { return Array(items.prefix(12)) }
        return items
            .filter { $0.title.localizedCaseInsensitiveContains(query) }
            .prefix(12)
            .map { $0 }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 11)).foregroundStyle(K.C.faint)
                TextField("Do something…", text: $query)
                    .textFieldStyle(.plain)
                    .font(K.F.ui(14))
                    .focused($focused)
                    .onSubmit { runSelected() }
                    .onChange(of: query) { selection = 0 }
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.md)

            if !hits.isEmpty {
                Hairline()
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(hits.enumerated()), id: \.element.id) { i, item in
                            HStack(spacing: K.S.sm) {
                                Text(item.title)
                                    .font(K.F.small)
                                    .foregroundStyle(K.C.text)
                                    .lineLimit(1).truncationMode(.middle)
                                Spacer(minLength: K.S.md)
                                Text(item.detail)
                                    .font(K.F.micro).foregroundStyle(K.C.faint)
                                    .lineLimit(1)
                                if i == selection {
                                    Image(systemName: "return")
                                        .font(.system(size: 8, weight: .bold))
                                        .foregroundStyle(K.C.faint)
                                }
                            }
                            .padding(.horizontal, K.S.md).padding(.vertical, 5)
                            .background(i == selection ? K.C.accent.opacity(0.18) : .clear)
                            .contentShape(Rectangle())
                            .onTapGesture { selection = i; runSelected() }
                        }
                    }
                }
                .frame(maxHeight: 320)
            }
        }
        .frame(width: 560)
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.lg))
        .overlay(RoundedRectangle(cornerRadius: K.R.lg).stroke(K.C.lineStrong, lineWidth: 1))
        .shadow(color: .black.opacity(0.35), radius: 30, y: 12)
        .onAppear { focused = true }
        .onKeyPress(.downArrow) { selection = min(selection + 1, max(hits.count - 1, 0)); return .handled }
        .onKeyPress(.upArrow) { selection = max(selection - 1, 0); return .handled }
        .onKeyPress(.escape) { open = false; return .handled }
    }

    private func runSelected() {
        guard selection < hits.count else { return }
        hits[selection].run()
        open = false
        query = ""
    }
}
