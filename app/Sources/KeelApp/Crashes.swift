import AppKit
import SwiftUI

/// The crash report macOS already wrote, surfaced in the window that crashed.
///
/// Sentry needs a key and a network; this needs neither. On launch, any `KeelApp-*.ips` or
/// `keel-*.ips` in `~/Library/Logs/DiagnosticReports` newer than the last one seen becomes a bar
/// with a **Copy report** button — so a tester on another machine can paste the actual trace
/// instead of describing it.
@MainActor
enum Crashes {
    private static let seenKey = "keel.crashes.seen"

    struct Report: Identifiable, Equatable {
        let url: URL
        let when: Date
        var id: String { url.path }
        var name: String { url.lastPathComponent }
        var text: String { (try? String(contentsOf: url, encoding: .utf8)) ?? "" }
    }

    static var reports: [URL] {
        let dir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs/DiagnosticReports")
        guard let all = try? FileManager.default.contentsOfDirectory(
            at: dir, includingPropertiesForKeys: [.contentModificationDateKey]) else { return [] }
        return all.filter {
            let n = $0.lastPathComponent
            return n.hasSuffix(".ips") && (n.hasPrefix("KeelApp-") || n.hasPrefix("keel-"))
        }
    }

    /// Reports written since the last launch that looked.
    static func unseen() -> [Report] {
        let since = UserDefaults.standard.object(forKey: seenKey) as? Date ?? .distantPast
        return reports.compactMap { url in
            let when = (try? url.resourceValues(forKeys: [.contentModificationDateKey]))?
                .contentModificationDate ?? .distantPast
            return when > since ? Report(url: url, when: when) : nil
        }
        .sorted { $0.when > $1.when }
    }

    static func markSeen() {
        UserDefaults.standard.set(Date(), forKey: seenKey)
    }
}

/// "Keel crashed last time."
struct CrashBar: View {
    let reports: [Crashes.Report]
    let dismiss: () -> Void
    @State private var copied = false

    var body: some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(K.F.tiny).foregroundStyle(K.C.warn)
            Text(reports.count == 1
                 ? "Keel crashed last time (\(reports[0].name))."
                 : "Keel crashed \(reports.count) times since you last looked.")
                .font(K.F.small).foregroundStyle(K.C.text)
            Text(Telemetry.crashReports && Telemetry.sentryConfigured
                 ? "The report was sent automatically; you can also copy it."
                 : "Copy the report and send it to the Keel team.")
                .font(K.F.micro).foregroundStyle(K.C.dim)
            Spacer()
            Button(copied ? "Copied" : "Copy report") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(
                    reports.map { "== \($0.name) ==\n" + $0.text }.joined(separator: "\n\n"),
                    forType: .string)
                copied = true
            }
            .buttonStyle(QuietButton(tone: K.C.accent))
            Button("Show in Finder") {
                NSWorkspace.shared.activateFileViewerSelecting(reports.map(\.url))
            }
            .buttonStyle(QuietButton())
            CloseButton(size: 10, label: "Dismiss") { dismiss() }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .background(K.C.warn.wash)
    }
}
