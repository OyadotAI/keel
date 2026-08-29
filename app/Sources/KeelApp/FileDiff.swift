import SwiftUI

/// One changed file, as a diff you can comment on.
///
/// The surface someone spends the most time reading, so the details are the design: line numbers
/// in a fixed gutter so the code column never shifts, backgrounds far weaker than the +/− glyphs
/// so forty changed lines stay readable, and word-level highlighting inside a changed line because
/// "this line changed" is rarely the answer to "what changed".
struct FileDiff: View {
    let path: String
    let model: SessionModel

    @State private var diff: Wire.Diff?
    @State private var open = false
    @State private var commenting: Int?
    @State private var draft = ""
    @State private var hoveringHeader = false

    private var adds: Int { diff?.hunks.flatMap(\.lines).count { $0.kind == "add" } ?? 0 }
    private var dels: Int { diff?.hunks.flatMap(\.lines).count { $0.kind == "del" } ?? 0 }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            if open, let diff {
                Hairline()
                ForEach(Array(diff.hunks.enumerated()), id: \.offset) { hi, hunk in
                    if hi > 0 { hunkSeparator(hunk.header) }
                    ForEach(Array(hunk.lines.enumerated()), id: \.offset) { _, line in
                        row(line)
                    }
                }
            }
        }
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        .task { diff = await model.diff(path) }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: open ? "chevron.down" : "chevron.right")
                .font(.system(size: 8, weight: .bold))
                .foregroundStyle(hoveringHeader ? K.C.dim : K.C.faint.opacity(0.6))
                .frame(width: 10)

            // Directory dimmed, filename not: you scan a diff list by filename.
            HStack(spacing: 0) {
                Text(directory).foregroundStyle(K.C.faint)
                Text(filename).foregroundStyle(K.C.text)
            }
            .font(K.F.mono(11.5, .medium))
            .lineLimit(1).truncationMode(.head)

            if diff?.untracked == true { Pill(text: "NEW", tone: .accent) }

            Spacer(minLength: K.S.sm)

            if diff != nil {
                DiffBar(adds: adds, dels: dels)
                Text("+\(adds)").font(K.F.mono(10)).foregroundStyle(K.C.add).monospacedDigit()
                Text("−\(dels)").font(K.F.mono(10)).foregroundStyle(K.C.del).monospacedDigit()
            }
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.sm)
        .background(hoveringHeader ? K.C.text.opacity(0.03) : .clear)
        .contentShape(Rectangle())
        .onHover { hoveringHeader = $0 }
        .onTapGesture { withAnimation(K.M.quick) { open.toggle() } }
        .contextMenu {
            Button("Attach as context") { model.mention(path) }
            Button("Reveal in Finder") { Task { await model.reveal(path) } }
            Divider()
            Button("Discard changes", role: .destructive) {
                Task { await model.gitAct("discard", path) }
            }
        }
    }

    private var filename: String { (path as NSString).lastPathComponent }
    private var directory: String {
        let dir = (path as NSString).deletingLastPathComponent
        return dir.isEmpty ? "" : dir + "/"
    }

    private func hunkSeparator(_ header: String) -> some View {
        Text(header)
            .font(K.F.mono(9.5))
            .foregroundStyle(K.C.faint)
            .padding(.horizontal, K.S.md)
            .padding(.vertical, 3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(K.C.well)
    }

    // MARK: Rows

    @ViewBuilder
    private func row(_ line: Wire.DiffLine) -> some View {
        let lineNo = line.new ?? line.old
        let key = "\(path):\(lineNo ?? -1)"
        let noted = model.notes[key] != nil

        HStack(alignment: .top, spacing: 0) {
            // The gutter is the comment affordance: clicking a line number starts a review on
            // every code host, so it starts one here.
            Text(line.old.map(String.init) ?? "")
                .frame(width: 34, alignment: .trailing)
            Text(line.new.map(String.init) ?? "")
                .frame(width: 34, alignment: .trailing)
                .padding(.trailing, K.S.sm)

            Text(marker(line.kind))
                .foregroundStyle(glyph(line.kind))
                .frame(width: 10, alignment: .leading)

            Text(line.text.isEmpty ? " " : line.text)
                .foregroundStyle(K.C.text)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)

            if noted {
                Image(systemName: "text.bubble.fill")
                    .font(.system(size: 8))
                    .foregroundStyle(K.C.accent)
                    .padding(.trailing, K.S.sm)
            }
        }
        .font(K.F.code)
        .foregroundStyle(K.C.faint)
        .padding(.leading, K.S.sm)
        .background(background(line.kind))
        .contentShape(Rectangle())
        .onTapGesture {
            guard let lineNo else { return }
            draft = model.notes[key] ?? ""
            withAnimation(K.M.quick) { commenting = commenting == lineNo ? nil : lineNo }
        }

        if commenting == lineNo, let lineNo {
            noteEditor(key: key, line: lineNo)
        }
    }

    private func noteEditor(key: String, line: Int) -> some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: "text.bubble")
                .font(.system(size: 9)).foregroundStyle(K.C.accent)
            TextField("A note for the agent…", text: $draft)
                .textFieldStyle(.plain)
                .font(K.F.small)
                .onSubmit { save(key: key) }
            Button("Add") { save(key: key) }.buttonStyle(QuietButton())
            Button("Cancel") { commenting = nil; draft = "" }.buttonStyle(QuietButton())
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.sm)
        .background(K.C.accent.opacity(0.07))
    }

    private func save(key: String) {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        model.notes[key] = text.isEmpty ? nil : text
        commenting = nil
        draft = ""
    }

    private func marker(_ kind: String) -> String {
        switch kind {
        case "add": "+"
        case "del": "−"
        default: " "
        }
    }

    private func glyph(_ kind: String) -> Color {
        switch kind {
        case "add": K.C.add
        case "del": K.C.del
        default: .clear
        }
    }

    private func background(_ kind: String) -> Color {
        switch kind {
        case "add": K.C.addBG
        case "del": K.C.delBG
        default: .clear
        }
    }
}

/// The five-segment bar every code host uses. Reads at a glance in a way two numbers do not.
struct DiffBar: View {
    let adds: Int
    let dels: Int

    private var filled: Int {
        let total = adds + dels
        guard total > 0 else { return 0 }
        return max(1, Int((Double(adds) / Double(total) * 5).rounded()))
    }

    var body: some View {
        HStack(spacing: 1.5) {
            ForEach(0..<5, id: \.self) { i in
                RoundedRectangle(cornerRadius: 0.5)
                    .fill(adds + dels == 0 ? K.C.line
                          : (i < filled ? K.C.add : K.C.del))
                    .frame(width: 5, height: 8)
            }
        }
    }
}
