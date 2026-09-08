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
        var id: String { detail + "|" + title }
        let title: String
        let detail: String
        /// The key that does the same thing without the palette, shown beside the row: the
        /// palette is where people learn the shortcuts, or it is where they never do.
        var shortcut: String? = nil
        let run: () -> Void
    }

    var items: [Item] {
        var out: [Item] = [
            Item(title: "New feature", detail: "Create an isolated branch and checkout",
                 shortcut: "⌘N") {
                NotificationCenter.default.post(name: .keelNewLane, object: nil)
            },
            Item(title: model.mode == "plan" ? "Switch to Auto" : "Switch to Plan",
                 detail: "what the agent is allowed to do", shortcut: "⇧⇥") {
                model.mode = model.mode == "plan" ? "acceptEdits" : "plan"
            },
            Item(title: "Next lane", detail: "the one below", shortcut: "⌘⇧]") {
                NotificationCenter.default.post(name: .keelNextLane, object: 1)
            },
            Item(title: "Toggle side panel", detail: "more room for the trace", shortcut: "⌘⇧E") {
                NotificationCenter.default.post(name: .keelTogglePanel, object: nil)
            },
            Item(title: "Trust this project…", detail: "stop asking about commands here",
                 shortcut: "⌘⇧T") {
                NotificationCenter.default.post(name: .keelTrust, object: nil)
            },
            Item(title: "Project setup", detail: "what this project still needs, with the fixes") {
                model.sheet = .setup
            },
            Item(title: "Settings", detail: "tools, permissions, devices", shortcut: "⌘,") {
                NotificationCenter.default.post(name: .keelSettings, object: nil)
            },
            Item(title: "Open project…", detail: "switches every lane", shortcut: "⌘O") {
                NotificationCenter.default.post(name: .keelOpenProject, object: nil)
            },
            Item(title: "Run the project's checks", detail: "the gate, on demand") {
                Task { await model.runGateNow() }
            },
            Item(title: "Terminal", detail: "full width, under both panes", shortcut: "⌘⌥T") {
                NotificationCenter.default.post(name: .keelToggleTerminal, object: nil)
            },
        ]
        if model.running || model.settling {
            out.insert(Item(title: "Stop the turn", detail: "Interrupt the active agent",
                            shortcut: "⌘.") { model.stop() }, at: 0)
        }
        out.insert(Item(title: "Focus the composer", detail: "Write your next instruction",
                        shortcut: "⌘L") {
            NotificationCenter.default.post(name: .keelFocusComposer, object: nil)
        }, at: 1)

        // Every panel and every stage. Nine of the ten panels and all three stages were reachable
        // only by clicking their icon — no shortcut, no menu item, and nothing here. In a
        // nine-icon rail that was the largest block of mouse-only surface in the app.
        for panel in SessionWindow.Panel.shown {
            out.append(Item(title: panel.title, detail: "panel") {
                NotificationCenter.default.post(name: .keelShowPanel, object: panel.rawValue)
            })
        }
        for stage in SessionWindow.Stage.shown {
            out.append(Item(title: stage.rawValue, detail: "stage") {
                NotificationCenter.default.post(name: .keelShowStage, object: stage.rawValue)
            })
        }

        // Git, which had no palette route at all — every verb was a click in one panel.
        if !model.changes.isEmpty {
            out.append(Item(title: "Commit everything uncommitted",
                            detail: "\(model.changes.count) file\(model.changes.count == 1 ? "" : "s")") {
                NotificationCenter.default.post(name: .keelShowPanel, object: "git")
            })
        }
        let unpushed = model.commits.filter { !$0.pushed }.count
        if unpushed > 0 {
            out.append(Item(title: "Push \(unpushed) commit\(unpushed == 1 ? "" : "s")",
                            detail: "only on this Mac until you do") {
                Task { await model.push() }
            })
        }
        if model.isRepo {
            out.append(Item(title: "Open a pull request…", detail: "gh, with your commits") {
                model.sheet = .pr
            })
        }

        // The lane you are in, and the ways out of it.
        out.append(Item(title: "Previous lane", detail: "the one above", shortcut: "⌘⇧[") {
            NotificationCenter.default.post(name: .keelNextLane, object: -1)
        })
        out.append(Item(title: "Review this task", detail: "the evidence, and the pull request") {
            NotificationCenter.default.post(name: .keelReviewTask, object: nil)
        })

        // What the panels open, so "add an MCP server" is a thing you can type rather than a thing
        // you have to know lives behind a panel you have not opened.
        out.append(Item(title: "Add an MCP server…", detail: "an external tool for the agent") {
            model.sheet = .mcp
        })
        out.append(Item(title: "Create a subagent…", detail: "a delegate with its own context") {
            model.sheet = .subagent
        })
        out.append(Item(title: "Browse skills and plugins…", detail: "for this repository") {
            model.sheet = .skills
        })
        if !model.notes.isEmpty {
            out.append(Item(title: "Send \(model.notes.count) review comment\(model.notes.count == 1 ? "" : "s")",
                            detail: "as the next prompt") {
                model.prompt = model.commentsPrompt()
                model.notes.removeAll()
            })
        }
        for (i, p) in model.pending.enumerated() {
            if p.isQuestion || p.isPlan {
                out.append(Item(title: p.isPlan ? "Review the agent's plan" : "Answer the agent's question",
                                detail: "Waiting for your decision") {
                    NotificationCenter.default.post(name: .keelFocusComposer, object: nil)
                })
                continue
            }
            let what = p.command.isEmpty ? p.tool : p.command
            out.append(Item(title: "Allow once: \(what)",
                            detail: "Only this invocation; no rule saved", shortcut: i == 0 ? "⌘⇧A" : nil) {
                model.answer(p, allow: true, scope: "once")
            })
            out.append(Item(title: "Approve for this project: \(what)",
                            detail: "a rule in .keel/permissions.json") {
                model.answer(p, allow: true, scope: "project")
            })
            // Approve had three routes here and Deny had none, which is the wrong way round: the
            // refusal is the half with consequences.
            out.append(Item(title: "Deny: \(what)",
                            detail: "the turn stops rather than working around it",
                            shortcut: i == 0 ? "⌘⇧D" : nil) {
                model.answer(p, allow: false, scope: "session")
            })
        }

        // Past sessions, which is the whole reason the History panel exists and had no way in.
        for session in model.sessions.prefix(20) {
            out.append(Item(title: session.title ?? session.id, detail: "past session") {
                Task { await model.lanes?.open(session: session.id) }
            })
        }
        // Recent projects, before files: switching project is a verb, and there are only ever a
        // handful of them.
        for path in Recents.paths.filter({ $0 != model.repoPath }) {
            out.append(Item(title: (path as NSString).lastPathComponent,
                            detail: "open project") {
                Task {
                    await model.open(project: path)
                    Recents.remember(path)
                    await model.lanes?.refreshShared()
                }
            })
        }

        // Files last: there are thousands, and they should not push the verbs off the list.
        // Not capped here — the match does the narrowing, so the four-hundred-and-first file
        // is as reachable as the first.
        for change in model.editedThisSession {
            out.append(Item(title: change.path, detail: "show the diff") {
                // Stage first: switching stage clears whatever is covering it, so setting the
                // diff before the switch would have the switch throw it straight back away.
                NotificationCenter.default.post(name: .keelShowStage, object: "Trace")
                model.show(diff: change.path)
            })
        }
        for f in model.files {
            out.append(Item(title: f, detail: "attach as context") { model.mention(f) })
        }
        return out
    }

    /// What is shown: recently run things first on an empty query, otherwise the best fuzzy
    /// matches. Substring matching is the thing every palette is criticised for — `nlb` should
    /// find "New lane on its own branch".
    private var hits: [Item] {
        let all = items
        guard !query.isEmpty else {
            let recent = Recent.titles
            let first = recent.compactMap { t in all.first { $0.title == t } }
            let rest = all.filter { !recent.contains($0.title) }
            return Array((first + rest).prefix(12))
        }
        return all
            .compactMap { item in Self.score(query, item: item).map { (item, $0) } }
            .sorted { $0.1 > $1.1 }
            .prefix(12)
            .map(\.0)
    }

    static func score(_ query: String, item: Item) -> Int? {
        // Prefer the name; searching "checkout" or "context" should also find the action
        // whose description explains that concept.
        let title = Fuzzy.score(query, in: item.title)
        let detail = Fuzzy.score(query, in: item.detail).map { $0 - 24 }
        return [title, detail].compactMap { $0 }.max()
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "magnifyingglass")
                    .font(K.F.micro).foregroundStyle(K.C.faint)
                TextField("Search commands, files, and sessions…", text: $query)
                    .textFieldStyle(.plain)
                    .font(K.F.reading)
                    .focused($focused)
                    .onSubmit { runSelected() }
                    .onChange(of: query) { selection = 0 }
                Text("esc").font(K.F.codeSmall).foregroundStyle(K.C.dim)
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.md)

            if !hits.isEmpty {
                Hairline()
                ScrollViewReader { proxy in
                  ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(hits.enumerated()), id: \.element.id) { i, item in
                            Button { selection = i; runSelected() } label: {
                              HStack(spacing: K.S.sm) {
                                Text(Fuzzy.highlight(query, in: item.title))
                                    .font(K.F.small)
                                    .foregroundStyle(K.C.text)
                                    .lineLimit(1).truncationMode(.middle)
                                Spacer(minLength: K.S.md)
                                Text(item.detail)
                                    .font(K.F.micro).foregroundStyle(K.C.dim)
                                    .lineLimit(1)
                                if let key = item.shortcut {
                                    Text(key).font(K.F.codeTiny).foregroundStyle(K.C.faint)
                                        .frame(width: 44, alignment: .trailing)
                                } else if i == selection {
                                    Image(systemName: "return")
                                        .font(K.F.tiny.weight(.bold))
                                        .foregroundStyle(K.C.faint)
                                        .frame(width: 44, alignment: .trailing)
                                } else {
                                    Color.clear.frame(width: 44, height: 1)
                                }
                            }
                            .padding(.horizontal, K.S.md).padding(.vertical, K.S.snug)
                            .background(i == selection ? K.C.tint : .clear)
                            .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .id(item.id)
                            .accessibilityLabel(item.title)
                            .accessibilityHint(item.detail)
                        }
                    }
                  }
                  .frame(maxHeight: 320)
                  .onChange(of: selection) {
                      if hits.indices.contains(selection) { proxy.scrollTo(hits[selection].id) }
                  }
                }
            } else {
                Hairline()
                VStack(alignment: .leading, spacing: K.S.xs) {
                    Text("No matches").font(K.F.body.weight(.semibold)).foregroundStyle(K.C.text)
                    Text("Try a command, a file name, or a recent session.")
                        .font(K.F.small).foregroundStyle(K.C.dim)
                }
                .frame(maxWidth: .infinity, alignment: .leading).padding(K.S.md)
            }
            Hairline()
            HStack(spacing: K.S.md) {
                Text("↑ ↓  Navigate")
                Text("↵  Run")
                Spacer()
                Text("⌘K  Commands")
            }
            .font(K.F.codeSmall).foregroundStyle(K.C.dim)
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
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
        let item = hits[selection]
        Recent.remember(item.title)
        item.run()
        open = false
        query = ""
    }
}

/// The last few things run from the palette, so an empty query shows them first.
enum Recent {
    private static let key = "keel.palette.recent"
    static var titles: [String] { UserDefaults.standard.stringArray(forKey: key) ?? [] }
    static func remember(_ title: String) {
        var t = titles.filter { $0 != title }
        t.insert(title, at: 0)
        UserDefaults.standard.set(Array(t.prefix(8)), forKey: key)
    }
}

/// Subsequence matching with a score, so `nlb` finds "New lane on its own branch" and a hit at
/// the start of a word beats one in the middle.
enum Fuzzy {
    /// `nil` when the query is not a subsequence. Higher is better.
    static func score(_ query: String, in text: String) -> Int? {
        let q = Array(query.lowercased()), t = Array(text.lowercased())
        guard !q.isEmpty else { return 0 }
        var qi = 0, score = 0, last = -2
        for (i, c) in t.enumerated() where qi < q.count && c == q[qi] {
            // Runs and word starts are what a person meant; scattered letters are what they
            // will accept.
            if i == last + 1 { score += 8 }
            if i == 0 || t[i - 1] == " " || t[i - 1] == "/" || t[i - 1] == "-" { score += 6 }
            score -= i / 4
            last = i
            qi += 1
        }
        guard qi == q.count else { return nil }
        // Shorter titles win ties: the exact verb over the file that contains it.
        return score * 4 - t.count / 8
    }

    /// The matched characters in bold, so the row says why it is there.
    static func highlight(_ query: String, in text: String) -> AttributedString {
        var out = AttributedString(text)
        // Walk the original's characters and lowercase each for the comparison. Lowercasing the
        // whole string first can change its character count, and an offset from one string
        // applied to the other walks off the end.
        let q = Array(query.lowercased()), t = Array(text)
        var qi = 0
        for (i, ch) in t.enumerated() where qi < q.count && Character(ch.lowercased()) == q[qi] {
            let lo = out.index(out.startIndex, offsetByCharacters: i)
            let hi = out.index(lo, offsetByCharacters: 1)
            out[lo..<hi].font = K.F.small.weight(.bold)
            qi += 1
        }
        return out
    }
}
