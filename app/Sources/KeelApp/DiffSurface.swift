import SwiftUI

/// One file's diff, full height, opened from the changes tree.
///
/// The diffs inside a turn answer "what did this turn do"; this answers "what does this file look
/// like right now", which is the other half of reviewing a change and the reason clicking a
/// changed file has to lead somewhere.
struct DiffSurface: View {
    let model: SessionModel
    let path: String

    @State private var diff: Wire.Diff?
    @State private var loading = true

    private var adds: Int { diff?.hunks.flatMap(\.lines).count { $0.kind == "add" } ?? 0 }
    private var dels: Int { diff?.hunks.flatMap(\.lines).count { $0.kind == "del" } ?? 0 }

    var body: some View {
        VStack(spacing: 0) {
            header
            Hairline()
            content
        }
        .background(K.C.bg)
        .task(id: "\(path)-\(model.diffTick)") {
            loading = true
            diff = await model.diff(path)
            loading = false
        }
    }

    private var header: some View {
        HStack(spacing: K.S.sm) {
            HStack(spacing: 0) {
                Text(directory).foregroundStyle(K.C.faint)
                Text(filename).foregroundStyle(K.C.text)
            }
            .font(K.F.mono(11.5, .medium))
            .lineLimit(1).truncationMode(.head)
            .textSelection(.enabled)

            if diff?.untracked == true { Pill(text: "NEW", tone: .accent) }

            Spacer(minLength: K.S.sm)

            if diff != nil {
                DiffBar(adds: adds, dels: dels)
                Text("+\(adds)").font(K.F.mono(10)).foregroundStyle(K.C.add).monospacedDigit()
                Text("−\(dels)").font(K.F.mono(10)).foregroundStyle(K.C.del).monospacedDigit()
            }

            Menu {
                Button("Attach as context") { model.mention(path) }
                Button("Reveal in Finder") { Task { await model.reveal(path) } }
                Divider()
                Button("Stage") { Task { await model.gitAct("stage", path) } }
                Button("Unstage") { Task { await model.gitAct("unstage", path) } }
                Button("Discard changes", role: .destructive) {
                    Task {
                        await model.gitAct("discard", path)
                        model.viewingDiff = nil
                    }
                }
            } label: {
                Image(systemName: "ellipsis").font(.system(size: 10))
            }
            .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
            .foregroundStyle(K.C.faint)
            .hint("Actions for this file")

            CloseButton(label: "Close diff") { model.viewingDiff = nil }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .background(K.C.surface)
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            centred("Reading…")
        } else if let diff, !diff.hunks.isEmpty {
            ScrollViewReader { proxy in
                // Vertical only. Two axes with a lazy stack centred the content in the pane and
                // let rows size to their own text; long lines truncate, whole line on hover.
                ScrollView(.vertical, showsIndicators: true) {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(diff.hunks.enumerated()), id: \.offset) { hi, hunk in
                            HunkHeader(header: hunk.header, index: hi, path: path,
                                       discardable: !diff.untracked, model: model)
                            let marks = Intraline.marks(hunk.lines)
                            ForEach(Array(hunk.lines.enumerated()), id: \.offset) { li, line in
                                DiffLineRow(line: line, mark: marks[li], path: path, model: model)
                            }
                        }
                        // Say where it ends. A pane that stops mid-table with nothing after it
                        // reads as cut off, whether or not it was.
                        let n = diff.hunks.reduce(0) { $0 + $1.lines.count }
                        HStack(spacing: K.S.sm) {
                            Rectangle().fill(K.C.line).frame(height: 1)
                            Text(diff.untracked ? "end of file · \(n) lines" : "end of changes · \(diff.hunks.count) hunk\(diff.hunks.count == 1 ? "" : "s")")
                                .font(K.F.micro).foregroundStyle(K.C.faint).fixedSize()
                            Rectangle().fill(K.C.line).frame(height: 1)
                        }
                        .padding(.horizontal, K.S.md).padding(.vertical, K.S.md)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                // ⌥↓ / ⌥↑ walk the hunks; ⌘⌥↓ / ⌘⌥↑ walk the changed files — on the pane, not
                // the scroll view, so the wheel is never contested.
                .background {
                    Color.clear
                        .focusable()
                        .onKeyPress(keys: [.downArrow, .upArrow], phases: .down) { press in
                            guard press.modifiers.contains(.option) else { return .ignored }
                            let by = press.key == .downArrow ? 1 : -1
                            return press.modifiers.contains(.command)
                                ? nextFile(by) : jump(proxy, by, of: diff.hunks.count)
                        }
                }
            }
        } else {
            // Not an error: an agent can revert a file, or the change can land in a commit while
            // you are looking at it.
            centred("This file matches HEAD — the change was committed or undone.")
        }
    }

    @State private var hunk = 0

    private func jump(_ proxy: ScrollViewProxy, _ by: Int, of count: Int) -> KeyPress.Result {
        guard count > 0 else { return .ignored }
        hunk = (hunk + by + count) % count
        withAnimation(K.M.settle) { proxy.scrollTo("hunk-\(path)-\(hunk)", anchor: .top) }
        return .handled
    }

    private func nextFile(_ by: Int) -> KeyPress.Result {
        let paths = model.changes.map(\.path)
        guard let i = paths.firstIndex(of: path), !paths.isEmpty else { return .ignored }
        model.viewingDiff = paths[(i + by + paths.count) % paths.count]
        hunk = 0
        return .handled
    }

    private func centred(_ text: String) -> some View {
        Text(text)
            .font(K.F.small).foregroundStyle(K.C.faint)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var filename: String { (path as NSString).lastPathComponent }
    private var directory: String {
        let d = (path as NSString).deletingLastPathComponent
        return d.isEmpty ? "" : d + "/"
    }
}

/// One line of a diff, with the gutter that starts a review.
struct DiffLineRow: View {
    let line: Wire.DiffLine
    var mark: Range<Int>? = nil
    let path: String
    let model: SessionModel
    @State private var editing = false
    @State private var draft = ""

    private var lineNo: Int? { line.new ?? line.old }
    private var key: String { "\(path):\(lineNo ?? -1)" }

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            Text(line.old.map(String.init) ?? "")
                .frame(width: 38, alignment: .trailing)
            Text(line.new.map(String.init) ?? "")
                .frame(width: 38, alignment: .trailing)
                .padding(.trailing, K.S.sm)
            Text(marker).foregroundStyle(glyph).frame(width: 10, alignment: .leading)
            Text(Intraline.styled(line.text, mark: mark, kind: line.kind))
                .foregroundStyle(K.C.text)
                .textSelection(.enabled)
                .lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .help(line.text.count > 80 ? line.text : "")
            if model.notes[key] != nil {
                Image(systemName: "text.bubble.fill")
                    .font(.system(size: 10)).foregroundStyle(K.C.accent)
                    .padding(.trailing, K.S.sm)
            }
        }
        .font(K.F.code)
        .foregroundStyle(K.C.faint)
        .padding(.leading, K.S.sm)
        .background(background)
        .contentShape(Rectangle())
        .asButton {
            guard lineNo != nil else { return }
            draft = model.notes[key] ?? ""
            withAnimation(K.M.quick) { editing.toggle() }
        }

        if editing {
            HStack(spacing: K.S.sm) {
                Image(systemName: "text.bubble").font(.system(size: 10))
                    .foregroundStyle(K.C.accent)
                TextField("A note for the agent…", text: $draft)
                    .textFieldStyle(.plain).font(K.F.small)
                    .onSubmit(save)
                Button("Add", action: save).buttonStyle(QuietButton())
                Button("Cancel") { editing = false }.buttonStyle(QuietButton())
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            .background(K.C.accent.opacity(0.07))
        }
    }

    private func save() {
        let t = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        model.notes[key] = t.isEmpty ? nil : t
        editing = false
        draft = ""
    }

    private var marker: String {
        switch line.kind { case "add": "+"; case "del": "−"; default: " " }
    }
    private var glyph: Color {
        switch line.kind { case "add": K.C.add; case "del": K.C.del; default: .clear }
    }
    private var background: Color {
        switch line.kind { case "add": K.C.addBG; case "del": K.C.delBG; default: .clear }
    }
}
