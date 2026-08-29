import AppKit
import Foundation
import PostHog
import Sentry

/// Crash reports and usage, behind one switch each.
///
/// Both SDKs sit behind this type so that a toggle in Settings actually silences them, and so
/// that no call site ever hands either of them a prompt, a path, or a repository name. Events
/// are verbs and counts: `turn_finished{gate: passed, files: 3}`. That is enough to see where
/// people stop, which is the whole question, and it is nothing anybody would mind sending.
///
/// Keys come from `Info.plist`, written by `packaging/build-app.sh` from the environment. A
/// build with no key has the SDK off, so a developer's own build reports nothing.
@MainActor
enum Telemetry {
    private static var info: [String: Any] { Bundle.main.infoDictionary ?? [:] }
    static var version: String { info["CFBundleShortVersionString"] as? String ?? "dev" }

    /// Crash reports go to Sentry. On by default; the welcome screen says so and Settings ›
    /// Privacy turns it off.
    static var crashReports: Bool {
        get { UserDefaults.standard.object(forKey: "keel.telemetry.crashes") as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: "keel.telemetry.crashes"); apply() }
    }
    /// Usage events go to PostHog. Same defaults, same switch.
    static var usage: Bool {
        get { UserDefaults.standard.object(forKey: "keel.telemetry.usage") as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: "keel.telemetry.usage"); apply() }
    }

    static var sentryConfigured: Bool { !(info["KeelSentryDSN"] as? String ?? "").isEmpty }
    static var posthogConfigured: Bool { !(info["KeelPostHogKey"] as? String ?? "").isEmpty }

    private static var started = false

    static func start() {
        guard !started else { return }
        started = true
        apply()
    }

    /// Bring both SDKs into line with the switches.
    private static func apply() {
        if crashReports, sentryConfigured, !sentryOn {
            SentrySDK.start { o in
                o.dsn = info["KeelSentryDSN"] as? String
                o.debug = ProcessInfo.processInfo.environment["KEEL_TELEMETRY_DEBUG"] == "1"
                o.releaseName = "keel@\(version)"
                o.environment = "production"
                o.enableAutoSessionTracking = true
                o.tracesSampleRate = 0
                // No screenshots or view hierarchies exist on macOS to attach; no PII either.
                o.sendDefaultPii = false
            }
            SentrySDK.configureScope { s in
                s.setTag(value: "keel", key: "app")
                s.setTag(value: "app", key: "component")
                s.setTag(value: version, key: "version")
            }
            sentryOn = true
        } else if !crashReports, sentryOn {
            SentrySDK.close()
            sentryOn = false
        }

        if posthogConfigured, !posthogOn {
            let config = PostHogConfig(
                apiKey: info["KeelPostHogKey"] as? String ?? "",
                host: info["KeelPostHogHost"] as? String ?? "https://us.i.posthog.com")
            config.captureApplicationLifecycleEvents = false
            config.captureScreenViews = false
            // A desktop app sends a handful of events an hour and is quit without warning;
            // batching twenty of them for thirty seconds is how the last ones never arrive.
            config.flushAt = 1
            config.flushIntervalSeconds = 5
            // `KEEL_TELEMETRY_DEBUG=1 dist/Keel.app/Contents/MacOS/KeelApp` prints every event
            // and every delivery, which is how "is it actually sending" gets answered.
            config.debug = ProcessInfo.processInfo.environment["KEEL_TELEMETRY_DEBUG"] == "1"
            PostHogSDK.shared.setup(config)
            PostHogSDK.shared.register([
                "app": "keel", "component": "app", "version": version,
                "os": ProcessInfo.processInfo.operatingSystemVersionString,
            ])
            posthogOn = true
        }
        if posthogOn {
            if usage { PostHogSDK.shared.optIn() } else { PostHogSDK.shared.optOut() }
        }
    }
    private static var sentryOn = false
    private static var posthogOn = false

    /// A usage event. Properties are counts and enum-like strings only.
    static func track(_ event: String, _ props: [String: Any] = [:]) {
        guard usage, posthogOn else { return }
        PostHogSDK.shared.capture(event, properties: props)
    }

    /// A line in the story a crash report tells: which pane, which action, no content.
    static func breadcrumb(_ message: String) {
        guard crashReports, sentryOn else { return }
        let crumb = Breadcrumb(level: .info, category: "ui")
        crumb.message = message
        SentrySDK.addBreadcrumb(crumb)
    }

    /// Before quitting: whatever is queued goes now.
    static func flush() {
        if posthogOn { PostHogSDK.shared.flush() }
        if sentryOn { SentrySDK.flush(timeout: 2) }
    }

    /// Something that is not a crash but is worth knowing happened to a tester: a turn that
    /// went silent, a question nobody saw. Tags only — a tool name, a count — never content.
    static func warn(_ message: String, _ tags: [String: String] = [:]) {
        guard crashReports, sentryOn else { return }
        SentrySDK.capture(message: message) { scope in
            scope.setLevel(.warning)
            for (k, v) in tags { scope.setTag(value: v, key: k) }
        }
    }

    /// Settings › Privacy › "Send a test report", so the pipeline can be seen working.
    static func sendTest() {
        guard sentryOn else { return }
        SentrySDK.capture(message: "Test report from Keel \(version)")
    }
}
