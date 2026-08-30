import SwiftUI

/// One changed file, as a diff you can comment on.
///
/// The surface someone spends the most time reading, so the details are the design: line numbers
/// in a fixed gutter so the code column never shifts, backgrounds far weaker than the +/− glyphs
/// so forty changed lines stay readable, and the changed *part* of a changed line marked, because
/// "this line changed" is rarely the answer to "what changed". Lines never wrap: wrapped code is a
/// lie about where the line breaks are, so long ones scroll sideways.
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
                // Full width, long lines truncated with the whole line on hover. A horizontal
                // scroll view here sized the diff to its longest line, and a diff of short
                // lines became a narrow strip down the left of the card.
                ForEach(Array(diff.hunks.enumerated()), id: \.offset) { hi, hunk in
                    HunkHeader(header: hunk.header, index: hi, path: path,
                               discardable: !diff.untracked, model: model)
                    let marks = Intraline.marks(hunk.lines)
                    // Ids unique across hunks: a lazy stack flattens nested ForEach, and two rows with
                            // the same id render as one — the second hunk's first rows were blank.
                            ForEach(Array(hunk.lines.enumerated()).map { ("\(hi)-\($0.offset)", $0.offset, $0.element) }, id: \.0) { _, li, line in
                        row(line, mark: marks[li])
                    }
                }
            }
        }
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        .task(id: model.diffTick) { diff = await model.diff(path) }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: open ? "chevron.down" : "chevron.right")
                .font(K.F.tiny.weight(.bold))
                .foregroundStyle(hoveringHeader ? K.C.dim : K.C.faint.opacity(0.6))
                .frame(width: 10)

            // Directory dimmed, filename not: you scan a diff list by filename.
            HStack(spacing: 0) {
                Text(directory).foregroundStyle(K.C.faint)
                Text(filename).foregroundStyle(K.C.text)
            }
            .font(K.F.codeSmall.weight(.medium))
            .lineLimit(1).truncationMode(.head)

            if diff?.untracked == true { Pill(text: "NEW", tone: .accent) }

            Spacer(minLength: K.S.sm)

            if diff != nil {
                DiffBar(adds: adds, dels: dels)
                Text("+\(adds)").font(K.F.codeTiny).foregroundStyle(K.C.add).monospacedDigit()
                Text("−\(dels)").font(K.F.codeTiny).foregroundStyle(K.C.del).monospacedDigit()
            }
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.sm)
        .background(hoveringHeader ? K.C.hover : .clear)
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

    // MARK: Rows

    @ViewBuilder
    private func row(_ line: Wire.DiffLine, mark: Range<Int>?) -> some View {
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

            Text(Intraline.styled(line.text, mark: mark, kind: line.kind))
                .foregroundStyle(K.C.text)
                .textSelection(.enabled)
                .lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .help(line.text.count > 80 ? line.text : "")

            if noted {
                Image(systemName: "text.bubble.fill")
                    .font(K.F.tiny)
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
                .font(K.F.tiny).foregroundStyle(K.C.accent)
            TextField("A note for the agent…", text: $draft)
                .textFieldStyle(.plain)
                .font(K.F.small)
                .onSubmit { save(key: key) }
            Button("Add") { save(key: key) }.buttonStyle(QuietButton())
            Button("Cancel") { commenting = nil; draft = "" }.buttonStyle(QuietButton())
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.sm)
        .background(K.C.accent.wash)
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

/// The `@@` line, with the one thing you can do to a hunk: throw it away on its own.
///
/// Six duplicate issues ask the CLI for this. Reviewing four hunks and wanting three of them is
/// the ordinary case, and discarding the file to get rid of one throws away the other three.
struct HunkHeader: View {
    let header: String
    let index: Int
    let path: String
    let discardable: Bool
    let model: SessionModel
    @State private var hovering = false

    var body: some View {
        HStack(spacing: K.S.sm) {
            Text(header).font(K.F.codeTiny).foregroundStyle(K.C.faint).lineLimit(1)
            Spacer()
            // Always present, faint until pointed at. A control that only exists on hover does
            // not exist for the keyboard, and does not exist for anyone who has not found it.
            if discardable {
                Button("discard hunk") {
                    Task { await model.gitAct("discard-hunk", path, hunk: index) }
                }
                .buttonStyle(QuietButton(tone: K.C.del))
                .opacity(hovering ? 1 : 0.55)
                .help("Put these lines back the way they were; the other hunks stay")
            }
        }
        .id("hunk-\(path)-\(index)")
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.tight)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(K.C.well)
        .onHover { hovering = $0 }
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
        HStack(spacing: K.S.hair) {
            ForEach(0..<5, id: \.self) { i in
                RoundedRectangle(cornerRadius: 0.5)
                    .fill(adds + dels == 0 ? K.C.line
                          : (i < filled ? K.C.add : K.C.del))
                    .frame(width: 5, height: 8)
            }
        }
    }
}

// MARK: - What changed inside the line

/// The changed span of a changed line, from the old and new versions side by side.
///
/// Common prefix and suffix by character, and the middle is what moved. Cheap enough to compute
/// per row, and right for the case that matters: a renamed identifier or a changed argument in an
/// otherwise identical line, where the whole-line tint says "this line" and nothing more.
enum Intraline {
    /// For each line in a hunk, the changed span — only for a `del` immediately followed by an
    /// `add` (or a run of each, paired in order). Everything else is `nil`: the whole line.
    static func marks(_ lines: [Wire.DiffLine]) -> [Range<Int>?] {
        var out = [Range<Int>?](repeating: nil, count: lines.count)
        var i = 0
        while i < lines.count {
            guard lines[i].kind == "del" else { i += 1; continue }
            var j = i
            while j < lines.count, lines[j].kind == "del" { j += 1 }
            var k = j
            while k < lines.count, lines[k].kind == "add" { k += 1 }
            // Pair the nth deletion with the nth addition; the unpaired tail stays whole.
            for n in 0..<min(j - i, k - j) {
                let (a, b) = span(lines[i + n].text, lines[j + n].text)
                out[i + n] = a
                out[j + n] = b
            }
            i = k
        }
        return out
    }

    /// The differing middle of two strings, as character offsets into each.
    static func span(_ old: String, _ new: String) -> (Range<Int>?, Range<Int>?) {
        let a = Array(old), b = Array(new)
        var head = 0
        while head < a.count, head < b.count, a[head] == b[head] { head += 1 }
        var tail = 0
        while tail < a.count - head, tail < b.count - head,
              a[a.count - 1 - tail] == b[b.count - 1 - tail] { tail += 1 }
        // A line that changed entirely gets no mark: highlighting all of it is highlighting
        // nothing.
        if head == 0 && tail == 0 { return (nil, nil) }
        let ra = head..<(a.count - tail), rb = head..<(b.count - tail)
        return (ra.isEmpty ? nil : ra, rb.isEmpty ? nil : rb)
    }

    /// The line with its changed span on a stronger ground.
    static func styled(_ text: String, mark: Range<Int>?, kind: String) -> AttributedString {
        var s = AttributedString(text.isEmpty ? " " : text)
        guard let mark, !text.isEmpty else { return s }
        let chars = Array(text)
        let lo = s.index(s.startIndex, offsetByCharacters: min(mark.lowerBound, chars.count))
        let hi = s.index(s.startIndex, offsetByCharacters: min(mark.upperBound, chars.count))
        s[lo..<hi].backgroundColor = (kind == "add" ? K.C.add : K.C.del).opacity(0.28)
        s[lo..<hi].font = K.F.codeSmall.weight(.semibold)
        return s
    }
}
