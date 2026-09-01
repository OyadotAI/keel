import AppKit
import SwiftUI

/// A block of the agent's reasoning, collapsed.
///
/// One per block rather than one merged blob for the turn: the CLI shows reasoning where it
/// happened, and three separate thoughts either side of two commands are not one thought.
struct ThinkingBlock: View {
    let block: Turn.Block
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Button { withAnimation(K.M.quick) { open.toggle() } } label: {
                HStack(spacing: K.S.tight) {
                    Image(systemName: "chevron.right")
                        .font(K.F.ui(7, .bold))
                        .rotationEffect(.degrees(open ? 90 : 0))
                    Text("thought")
                    Text(block.text.words).monospacedDigit()
                }
                .font(K.F.micro)
                .foregroundStyle(K.C.faint)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if open {
                Text(block.text)
                    .font(K.F.codeSmall).italic()
                    .foregroundStyle(K.C.faint)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.leading, K.S.sm)
                    .overlay(alignment: .leading) { Rectangle().fill(K.C.line).frame(width: 2) }
                    .transition(.opacity)
            }
        }
        .padding(.leading, K.S.xs)
    }
}

/// One tool call, where it happened in the conversation.
///
/// Collapsed it is a line: what ran, and how long it took. Open it is what the CLI prints — the
/// whole command, the whole patch, the whole subagent prompt — and what came back. Keel used to
/// keep one field of the input, first line only, cut at 160 characters, so a heredoc showed the
/// word `cat` and an `Edit` showed a path and never its hunks.
struct CallRow: View {
    let call: Turn.Call
    @State private var userSet: Bool?

    private var open: Bool { userSet ?? false }
    private var hasDetail: Bool { !call.input.isEmpty || !call.output.isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Button {
                guard hasDetail else { return }
                withAnimation(K.M.quick) { userSet = !open }
            } label: {
                HStack(spacing: K.S.sm) {
                    Image(systemName: "chevron.right")
                        .font(K.F.ui(7, .bold))
                        .rotationEffect(.degrees(open ? 90 : 0))
                        .foregroundStyle(hasDetail ? K.C.faint : .clear)
                    CallGlyph(risk: call.risk, failed: call.failed, running: call.running)
                    Text(call.tool)
                        .font(K.F.micro.weight(.semibold))
                        .foregroundStyle(call.failed ? K.C.del : K.C.dim)
                    Text(call.subject)
                        .font(K.F.codeSmall)
                        .foregroundStyle(K.C.faint)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: K.S.xs)
                    Elapsed(started: call.started, duration: call.duration, running: call.running)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if open {
                VStack(alignment: .leading, spacing: K.S.sm) {
                    ForEach(Self.parts(of: call), id: \.label) { part in
                        VStack(alignment: .leading, spacing: K.S.xxs) {
                            Text(part.label).sectionLabel().foregroundStyle(K.C.faint)
                            CodeBlock(language: part.language, text: part.text)
                        }
                    }
                    if !call.output.isEmpty {
                        VStack(alignment: .leading, spacing: K.S.xxs) {
                            Text(call.failed ? "ERROR" : "OUTPUT")
                                .sectionLabel()
                                .foregroundStyle(call.failed ? K.C.del : K.C.faint)
                            Lines(text: call.output)
                        }
                    }
                }
                .padding(.leading, K.S.lg)
                .padding(.trailing, K.S.lg)
                .transition(.opacity)
            }
        }
        .padding(.leading, K.S.xs)
    }

    /// What was passed to the tool, as the parts worth reading separately.
    ///
    /// The known tools get their own shape because a `command` and a patch are read differently;
    /// everything else falls back to its JSON, which is still the whole thing rather than a
    /// hundred-and-sixty-character guess at which field mattered.
    struct Part { let label: String; let language: String?; let text: String }

    static func parts(of call: Turn.Call) -> [Part] {
        func str(_ key: String) -> String? {
            guard case .string(let v)? = call.input[key], !v.isEmpty else { return nil }
            return v
        }
        var out: [Part] = []
        if let why = str("description"), call.tool != "Task" {
            out.append(Part(label: "WHY", language: nil, text: why))
        }
        switch call.tool {
        case "Bash":
            if let c = str("command") { out.append(Part(label: "COMMAND", language: "sh", text: c)) }
        case "Edit", "Update":
            if let p = str("file_path") { out.append(Part(label: "FILE", language: nil, text: p)) }
            if let o = str("old_string") { out.append(Part(label: "REPLACING", language: nil, text: o)) }
            if let n = str("new_string") { out.append(Part(label: "WITH", language: nil, text: n)) }
        case "Write":
            if let p = str("file_path") { out.append(Part(label: "FILE", language: nil, text: p)) }
            if let c = str("content") { out.append(Part(label: "CONTENTS", language: nil, text: c)) }
        case "Task":
            if let d = str("description") { out.append(Part(label: "TASK", language: nil, text: d)) }
            if let p = str("prompt") { out.append(Part(label: "PROMPT", language: nil, text: p)) }
        case "Read", "Glob", "Grep":
            for key in ["file_path", "path", "pattern", "glob"] {
                if let v = str(key) { out.append(Part(label: key.uppercased(), language: nil, text: v)) }
            }
        default:
            break
        }
        // Nothing recognised, or a tool with more to it than the cases above: the whole input.
        if out.isEmpty, !call.input.isEmpty {
            out.append(Part(label: "INPUT", language: "json", text: call.input.pretty))
        }
        return out
    }
}

/// Long output, a line at a time.
///
/// A `Text` holding twenty thousand characters has to be laid out whole to size the scroll view
/// around it, and CoreText encoding that much on the main thread is a measurable hang — the same
/// one `String.capped` was written for. Lines in a lazy stack are measured as they are reached.
struct Lines: View {
    let text: String
    /// Enough to see what happened; the rest is one click away and almost never wanted.
    static let shown = 300
    @State private var all = false

    private var lines: [String] { text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init) }

    var body: some View {
        let lines = lines
        let cut = all ? lines : Array(lines.prefix(Self.shown))
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(cut.enumerated()), id: \.offset) { _, line in
                        Text(line.capped(2_000))
                            .font(K.F.codeSmall)
                            .foregroundStyle(K.C.dim)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(K.S.sm)
            }
            .frame(maxHeight: 260)
            if lines.count > Self.shown {
                Button(all ? "Show less" : "Show all \(lines.count) lines") {
                    withAnimation(K.M.quick) { all.toggle() }
                }
                .buttonStyle(.plain)
                .font(K.F.micro)
                .foregroundStyle(K.C.accent)
                .padding(.horizontal, K.S.sm).padding(.bottom, K.S.xs)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
    }
}

extension String {
    /// "1,204 words", for a label on something collapsed.
    var words: String {
        let n = split(whereSeparator: \.isWhitespace).count
        return "\(n) word\(n == 1 ? "" : "s")"
    }
}

extension [String: JSONValue] {
    /// The input as it was sent, indented. The fallback for a tool Keel has no shape for — and
    /// the whole of it, because the point is that nothing is hidden.
    var pretty: String {
        let pairs = sorted { $0.key < $1.key }
            .map { "  \"\($0.key)\": \($0.value.pretty(indent: 1))" }
        return "{\n" + pairs.joined(separator: ",\n") + "\n}"
    }
}
