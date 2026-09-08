import Foundation

/// Recover explicit write targets from successful recorded shell calls, without executing code
/// or consulting today's filesystem. This is deliberately not a shell/Python interpreter:
/// literal cat redirections and straight-line Python heredoc writes are supported; dynamic
/// paths, arbitrary scripts, branches and loops remain in the trace, not guessed into this list.
enum RecordedFileWrites {
    private static let quoted = #"(?:'[^'\\\r\n]*'|"[^"\\\r\n]*")"#
    private static let word = #"(?:'[^'\r\n]*'|"[^"\r\n]*"|[^\s;&|<>$`\\()]+)"#
    private static let identifier = #"[A-Za-z_][A-Za-z_0-9]*"#

    static func paths(in command: String, cwd: String?) -> [String] {
        // Bounded, including commands containing megabytes of pasted source.
        guard command.utf8.count <= 512 * 1024,
              command.contains(">") || command.contains(".write") || command.contains("open(")
        else { return [] }
        var directory = cwd
        var delimiter: String?
        var python = false
        var triple: String?
        var bindings: [String: String] = [:]
        var result: [String] = []
        var seen = Set<String>()

        func add(_ raw: String?) {
            guard result.count < 40, let raw, let path = resolve(raw, cwd: directory),
                  !path.hasPrefix("/dev/"), seen.insert(path).inserted else { return }
            result.append(path)
        }

        for linePart in command.split(separator: "\n", omittingEmptySubsequences: false) {
            let original = String(linePart)
            if let end = delimiter {
                if original.trimmingCharacters(in: .whitespaces) == end {
                    delimiter = nil; python = false; triple = nil; bindings = [:]
                    continue
                }
                guard python else { continue } // A document's contents are never commands.
                let wasInString = triple != nil
                updateTripleQuotes(original, active: &triple)
                guard !wasInString else { continue }
                let stripped = original.trimmingCharacters(in: .whitespaces)
                if match(#"^(?:os\.)?chdir\s*\("#, stripped) != nil {
                    // A script changing directories needs interpretation, not the shell's cwd.
                    python = false
                    continue
                }
                if original.hasPrefix(" ") || original.hasPrefix("\t") {
                    if let assignment = match("^(\(identifier))\\s*=", stripped) {
                        // We do not know which branch ran. Forget rather than reuse a stale path.
                        bindings[assignment[1]] = nil
                    }
                    continue
                }

                // Bind only a complete literal or Path(literal). Any other reassignment forgets
                // the binding so an old value cannot be attributed to a later dynamic write.
                if let assignment = match("^(\(identifier))\\s*=\\s*(.*)$", original) {
                    bindings[assignment[1]] = literal(assignment[2])
                }
                let target = "(?:\(quoted)|\(identifier))"
                if let opened = match("^(?:with\\s+)?open\\(\\s*(\(target))\\s*,\\s*(?:mode\\s*=\\s*)?(\(quoted))", original),
                   let mode = literal(opened[2]), let first = mode.first, "wax".contains(first) {
                    add(literal(opened[1]) ?? bindings[opened[1]])
                }
                if let written = match("^(\(identifier))\\.(?:write_text|write_bytes)\\s*\\(", original) {
                    add(bindings[written[1]])
                }
                if let written = match("^Path\\(\\s*(\(quoted))\\s*\\)\\.(?:write_text|write_bytes)\\s*\\(", original) {
                    add(literal(written[1]))
                }
                continue
            }

            var line = original.trimmingCharacters(in: .whitespaces)
            // The common `cd /checkout && python3 - <<'PY'` form changes relative targets.
            if let cd = match("^cd\\s+(\(word))\\s*&&\\s*(.*)$", line) {
                guard let next = resolve(unquote(cd[1]), cwd: directory) else { return result }
                directory = next
                line = cd[2]
            } else if let cd = match("^cd\\s+(\(word))\\s*$", line) {
                guard let next = resolve(unquote(cd[1]), cwd: directory) else { return result }
                directory = next
                continue
            } else if line.hasPrefix("cd ") {
                // Unknown shell expansion: do not resolve relative targets against the old cwd.
                return result
            }

            if let redirected = match("^cat\\s+(?:1)?>>?\\s*(\(word))(?:\\s|$)", line) {
                add(unquote(redirected[1]))
            }
            if let heredoc = match(#"<<-?\s*('([^']+)'|"([^"]+)"|([A-Za-z_][A-Za-z_0-9]*))"#, line) {
                delimiter = unquote(heredoc[1])
                python = match(#"^python(?:3(?:\.\d+)?)?\s+(?:-\s+)?<<"#, line) != nil
            }
        }
        return result
    }

    private static func literal(_ expression: String) -> String? {
        let text = expression.trimmingCharacters(in: .whitespaces)
        if let path = match("^Path\\(\\s*(\(quoted))\\s*\\)$", text) { return unquote(path[1]) }
        guard match("^\(quoted)$", text) != nil else { return nil }
        return unquote(text)
    }

    private static func unquote(_ value: String) -> String {
        guard let first = value.first, first == "'" || first == "\"", value.last == first else { return value }
        return String(value.dropFirst().dropLast())
    }

    private static func resolve(_ raw: String, cwd: String?) -> String? {
        guard !raw.isEmpty, !raw.contains("://"),
              raw.rangeOfCharacter(from: CharacterSet(charactersIn: "$`\\\n\r\0")) == nil,
              !raw.hasPrefix("~") else { return nil }
        if raw.hasPrefix("/") { return (raw as NSString).standardizingPath }
        guard let cwd else { return (raw as NSString).standardizingPath }
        return ((cwd as NSString).appendingPathComponent(raw) as NSString).standardizingPath
    }

    private static func match(_ pattern: String, _ text: String) -> [String]? {
        guard let regex = try? NSRegularExpression(pattern: pattern),
              let match = regex.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)) else { return nil }
        return (0..<match.numberOfRanges).map { index in
            Range(match.range(at: index), in: text).map { String(text[$0]) } ?? ""
        }
    }

    /// Ignore Python multiline string bodies, including source pasted into replacement strings.
    private static func updateTripleQuotes(_ line: String, active: inout String?) {
        var rest = line[...]
        while !rest.isEmpty {
            if let delimiter = active {
                guard let end = rest.range(of: delimiter) else { return }
                active = nil; rest = rest[end.upperBound...]
            } else {
                let candidates = ["\"\"\"", "'''"].compactMap { quote in
                    rest.range(of: quote).map { (quote, $0) }
                }
                guard let first = candidates.min(by: { $0.1.lowerBound < $1.1.lowerBound }) else { return }
                active = first.0; rest = rest[first.1.upperBound...]
            }
        }
    }
}
