import AppKit
import SwiftUI

/// The agent's replies, rendered as what they are.
///
/// Claude Code emits Markdown: headings, lists, and fenced code. Preformatted text throws all of
/// that away, and a wall of monospace is the single fastest way to make a reply unreadable.
///
/// Block structure is parsed here; inline emphasis and `code` are handed to `AttributedString`,
/// which already does that correctly. Deliberately not a full CommonMark implementation — tables,
/// footnotes and nested blockquotes are not what an agent writes back about a code change, and a
/// parser that handles them is a parser to maintain.
struct Markdown: View {
    let source: String

    init(_ source: String) { self.source = source }

    var body: some View {
        let blocks = Self.cachedBlocks(source)
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { index, block in
                view(for: block)
                    .padding(.top, spacing(before: block, at: index))
            }
        }
        // Applied at the root so every block inherits it. Only the code blocks had it before, so
        // the actual answer — the part worth quoting — was the one part you could not select.
        .textSelection(.enabled)
    }

    @ViewBuilder
    private func view(for block: Block) -> some View {
        switch block {
        case .heading(let level, let text):
            Text(inline(text))
                .font(K.F.ui(level == 1 ? 20 : (level == 2 ? 17 : 14), .semibold))
                .foregroundStyle(K.C.text)
                .lineSpacing(2)

        case .paragraph(let text):
            Text(inline(text))
                .font(K.F.ui(14))
                .foregroundStyle(K.C.text)
                .lineSpacing(3)
                .fixedSize(horizontal: false, vertical: true)

        case .bullets(let items):
            VStack(alignment: .leading, spacing: 7) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                    HStack(alignment: .firstTextBaseline, spacing: K.S.sm) {
                        Text(item.marker)
                            .font(K.F.mono(12))
                            .foregroundStyle(K.C.faint)
                            .frame(minWidth: 18, alignment: .trailing)
                        Text(inline(item.text))
                            .font(K.F.ui(14))
                            .foregroundStyle(K.C.text)
                            .lineSpacing(3)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }

        case .code(let language, let text):
            CodeBlock(language: language, text: text)

        case .table(let rows):
            Table(rows: rows)

        case .rule:
            Rectangle().fill(K.C.line).frame(height: 1).padding(.vertical, K.S.xs)
        }
    }

    /// Space follows meaning rather than treating every Markdown block as an identical row.
    /// Headings open a new section; prose and lists within a section stay visibly connected.
    private func spacing(before block: Block, at index: Int) -> CGFloat {
        guard index > 0 else { return 0 }
        return switch block {
        case .heading: K.S.xl
        case .rule: K.S.lg
        case .code, .table: K.S.md
        case .bullets: K.S.sm
        case .paragraph: K.S.md
        }
    }

    /// Inline emphasis and `code`. Failure falls back to the literal text rather than dropping it:
    /// an unparseable reply must still be readable.
    private func inline(_ s: String) -> AttributedString {
        (try? AttributedString(
            markdown: s,
            options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(s)
    }

    // MARK: - Blocks

    enum Block {
        case heading(Int, String)
        case paragraph(String)
        case bullets([Item])
        case code(String?, String)
        case table([[String]])
        case rule

        struct Item { let marker: String; let text: String }
    }

    /// The parse, memoised.
    ///
    /// `body` runs on every text delta while a reply streams, and this parses the whole string
    /// each time — so a ten-kilobyte answer was re-parsed thousands of times on the way in, once
    /// per frame, which is what made the pane stutter under a scroll.
    ///
    /// Keyed by the text itself and bounded, rather than a single slot: several turns render in
    /// the same pass, so a one-entry cache would be evicted by its neighbour every time and buy
    /// nothing.
    @MainActor
    private static var cache: [String: [Block]] = [:]
    @MainActor
    private static var order: [String] = []

    @MainActor
    static func cachedBlocks(_ source: String) -> [Block] {
        if let hit = cache[source] { return hit }
        let parsed = blocks(source)
        cache[source] = parsed
        order.append(source)
        // A streaming reply produces one new string per delta, so this would otherwise grow to
        // hold every intermediate state of every answer.
        if order.count > 40 {
            cache.removeValue(forKey: order.removeFirst())
        }
        return parsed
    }

    static func blocks(_ source: String) -> [Block] {
        var out: [Block] = []
        var paragraph: [String] = []
        var bullets: [Block.Item] = []

        func flushParagraph() {
            if !paragraph.isEmpty {
                // Agents often use a short colon-ended line as a section lead-in before a list.
                // It is semantic hierarchy even when the model omitted Markdown hashes.
                if paragraph.count == 1, paragraph[0].hasSuffix(":") {
                    out.append(.heading(3, paragraph[0]))
                } else {
                    out.append(.paragraph(paragraph.joined(separator: " ")))
                }
                paragraph = []
            }
        }
        func flushBullets() {
            if !bullets.isEmpty { out.append(.bullets(bullets)); bullets = [] }
        }
        func flushAll() { flushParagraph(); flushBullets() }

        var lines = source.split(separator: "\n", omittingEmptySubsequences: false)
            .map(String.init)[...]

        while let line = lines.first {
            lines = lines.dropFirst()
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            // A fence runs to its closing fence, or to the end — an unterminated block is what a
            // stream looks like halfway through, and it has to render rather than vanish.
            if trimmed.hasPrefix("```") {
                flushAll()
                let language = String(trimmed.dropFirst(3)).trimmingCharacters(in: .whitespaces)
                var body: [String] = []
                while let next = lines.first {
                    lines = lines.dropFirst()
                    if next.trimmingCharacters(in: .whitespaces).hasPrefix("```") { break }
                    body.append(next)
                }
                out.append(.code(language.isEmpty ? nil : language,
                                 body.joined(separator: "\n")))
                continue
            }

            if trimmed.isEmpty { flushAll(); continue }

            // A table is a pipe row followed by a `|---|` separator. Rendered as a paragraph it is
            // a wall of punctuation, which is what a metrics answer looked like before this.
            if trimmed.hasPrefix("|"), let next = lines.first,
               isSeparator(next.trimmingCharacters(in: .whitespaces)) {
                flushAll()
                lines = lines.dropFirst()
                var rows = [cells(trimmed)]
                while let row = lines.first,
                      row.trimmingCharacters(in: .whitespaces).hasPrefix("|") {
                    lines = lines.dropFirst()
                    rows.append(cells(row.trimmingCharacters(in: .whitespaces)))
                }
                out.append(.table(rows))
                continue
            }

            if trimmed == "---" || trimmed == "***" || trimmed == "___" {
                flushAll()
                out.append(.rule)
                continue
            }

            if trimmed.hasPrefix("#") {
                flushAll()
                let level = trimmed.prefix(while: { $0 == "#" }).count
                out.append(.heading(min(level, 3),
                                    String(trimmed.dropFirst(level)).trimmingCharacters(in: .whitespaces)))
                continue
            }

            if let item = bullet(trimmed) {
                flushParagraph()
                bullets.append(item)
                continue
            }

            flushBullets()
            paragraph.append(trimmed)
        }

        flushAll()
        return out
    }

    /// `|---|:--:|` and friends: the row that turns the one above it into a header.
    private static func isSeparator(_ line: String) -> Bool {
        guard line.hasPrefix("|") else { return false }
        let body = line.filter { !" |".contains($0) }
        return !body.isEmpty && body.allSatisfy { $0 == "-" || $0 == ":" }
    }

    private static func cells(_ row: String) -> [String] {
        var r = row
        if r.hasPrefix("|") { r.removeFirst() }
        if r.hasSuffix("|") { r.removeLast() }
        return r.split(separator: "|", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
    }

    /// `- `, `* `, and `1. ` — the three an agent actually writes.
    private static func bullet(_ line: String) -> Block.Item? {
        for marker in ["- ", "* ", "+ "] where line.hasPrefix(marker) {
            return .init(marker: "•", text: String(line.dropFirst(marker.count)))
        }
        let digits = line.prefix(while: \.isNumber)
        if !digits.isEmpty, line.dropFirst(digits.count).hasPrefix(". ") {
            return .init(marker: "\(digits).",
                         text: String(line.dropFirst(digits.count + 2)))
        }
        return nil
    }
}

/// A fenced block: scrollable sideways rather than wrapped, because wrapped code is a lie about
/// where the line breaks are, and copyable, because that is what it is for.
struct CodeBlock: View {
    let language: String?
    let text: String
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if language != nil || true {
                HStack {
                    if let language {
                        Text(language).font(K.F.mono(10)).foregroundStyle(K.C.faint)
                    }
                    Spacer()
                    Button(copied ? "copied" : "copy") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(text, forType: .string)
                        copied = true
                        Task { try? await Task.sleep(for: .seconds(1.5)); copied = false }
                    }
                    .buttonStyle(.plain)
                    .font(K.F.mono(10))
                    .foregroundStyle(copied ? K.C.add : K.C.faint)
                    .padding(.horizontal, 4).padding(.vertical, 2)
                    .contentShape(Rectangle())
                }
                .padding(.horizontal, K.S.sm)
                .padding(.vertical, 3)
            }

            ScrollView(.horizontal, showsIndicators: false) {
                Text(text)
                    .font(K.F.code)
                    .foregroundStyle(K.C.text)
                    .textSelection(.enabled)
                    .padding(.horizontal, K.S.sm)
                    .padding(.bottom, K.S.sm)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
    }
}


/// A Markdown table, as a table.
///
/// Columns size to their widest cell and the whole thing scrolls sideways rather than wrapping,
/// because a wrapped table stops being one. Numeric columns are right-aligned with tabular figures
/// so a column of counts lines up on its digits — which is the only reason to put numbers in a
/// table at all.
struct Table: View {
    let rows: [[String]]
    @State private var copied = false

    private var columns: Int { rows.map(\.count).max() ?? 0 }

    /// A column is numeric when every body cell in it reads as a number.
    private func numeric(_ column: Int) -> Bool {
        let body = rows.dropFirst().compactMap { $0.indices.contains(column) ? $0[column] : nil }
            .filter { !$0.isEmpty }
        guard !body.isEmpty else { return false }
        return body.allSatisfy { cell in
            cell.allSatisfy { $0.isNumber || ".,-+%$s".contains($0) }
        }
    }

    /// The table back as Markdown, so what is copied pastes as a table anywhere.
    private var markdown: String {
        guard let head = rows.first else { return "" }
        let sep = "| " + head.map { _ in "---" }.joined(separator: " | ") + " |"
        let line = { (r: [String]) in "| " + r.map { $0.replacingOccurrences(of: "|", with: "\\|") }.joined(separator: " | ") + " |" }
        return ([line(head), sep] + rows.dropFirst().map(line)).joined(separator: "\n")
    }

    var body: some View {
        // A Grid, so every row shares the column widths — a stack of HStacks sized each row
        // to its own cells and nothing lined up. Text wraps; a table that scrolls sideways
        // is a table nobody reads to the end.
        Grid(alignment: .topLeading, horizontalSpacing: 0, verticalSpacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.offset) { i, row in
                GridRow {
                    ForEach(0..<columns, id: \.self) { c in
                        Text(row.indices.contains(c) ? row[c] : "")
                            .font(i == 0 ? K.F.small.weight(.semibold) : K.F.small)
                            .foregroundStyle(i == 0 ? K.C.dim : K.C.text)
                            .monospacedDigit()
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: numeric(c) ? .trailing : .leading)
                            .gridColumnAlignment(numeric(c) ? .trailing : .leading)
                            .padding(.horizontal, K.S.sm)
                            .padding(.vertical, 4)
                            .background(i > 0 && i.isMultiple(of: 2) ? K.C.text.opacity(0.03) : .clear)
                    }
                }
                if i == 0 {
                    GridRow { Rectangle().fill(K.C.line).frame(height: 1).gridCellColumns(columns) }
                }
            }
        }
        .padding(.vertical, 2)
        .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        .overlay(alignment: .topTrailing) {
            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(markdown, forType: .string)
                copied = true
                Task { try? await Task.sleep(for: .seconds(1.5)); copied = false }
            } label: {
                Image(systemName: copied ? "checkmark" : "doc.on.doc").font(.system(size: 10, weight: .bold))
                    .foregroundStyle(copied ? K.C.add : K.C.faint)
                    .padding(4)
            }
            .buttonStyle(.plain)
            .help("Copy the table as Markdown")
        }
    }
}
