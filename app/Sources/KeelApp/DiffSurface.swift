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
        .task(id: path) {
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

            CloseButton { model.viewingDiff = nil }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .background(K.C.surface)
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            centred("Reading…")
        } else if let diff, !diff.hunks.isEmpty {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(diff.hunks.enumerated()), id: \.offset) { hi, hunk in
                        if hi > 0 || diff.hunks.count > 1 {
                            Text(hunk.header)
                                .font(K.F.mono(9.5)).foregroundStyle(K.C.faint)
                                .padding(.horizontal, K.S.md).padding(.vertical, 3)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .background(K.C.well)
                        }
                        ForEach(Array(hunk.lines.enumerated()), id: \.offset) { _, line in
                            DiffLineRow(line: line, path: path, model: model)
                        }
                    }
                }
            }
        } else {
            // Not an error: an agent can revert a file, or the change can land in a commit while
            // you are looking at it.
            centred("This file matches HEAD — the change was committed or undone.")
        }
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
            Text(line.text.isEmpty ? " " : line.text)
                .foregroundStyle(K.C.text)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            if model.notes[key] != nil {
                Image(systemName: "text.bubble.fill")
                    .font(.system(size: 8)).foregroundStyle(K.C.accent)
                    .padding(.trailing, K.S.sm)
            }
        }
        .font(K.F.code)
        .foregroundStyle(K.C.faint)
        .padding(.leading, K.S.sm)
        .background(background)
        .contentShape(Rectangle())
        .onTapGesture {
            guard lineNo != nil else { return }
            draft = model.notes[key] ?? ""
            withAnimation(K.M.quick) { editing.toggle() }
        }

        if editing {
            HStack(spacing: K.S.sm) {
                Image(systemName: "text.bubble").font(.system(size: 9))
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
