// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "KeelApp",
    platforms: [.macOS(.v15)],
    dependencies: [
        // A terminal emulator is not a thing to write. Orca built its own and 678 of its issues
        // mention the terminal — garbled output, IME breakage in Korean and Chinese, escape
        // sequences leaking into the shell. SwiftTerm is the mature Swift one and its IME handling
        // is the specific reason to take it.
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", from: "1.2.0"),
        // Updates, crashes and usage. Each is one dependency because building any of them is
        // a year of somebody else's edge cases: Sparkle's EdDSA-signed appcasts, Sentry's
        // in-process crash handler that survives the crash, PostHog's batching and retries.
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.6.0"),
        .package(url: "https://github.com/getsentry/sentry-cocoa", from: "8.40.0"),
        .package(url: "https://github.com/PostHog/posthog-ios", from: "3.15.0"),
    ],
    targets: [
        .executableTarget(
            name: "KeelApp",
            dependencies: [
                .product(name: "SwiftTerm", package: "SwiftTerm"),
                .product(name: "Sparkle", package: "Sparkle"),
                .product(name: "Sentry", package: "sentry-cocoa"),
                .product(name: "PostHog", package: "posthog-ios"),
            ],
            path: "Sources/KeelApp",
            // The picker is JavaScript that runs inside someone else's page, so it ships as a
            // resource rather than a Swift string literal — escaping a hundred lines of JS into
            // source is how it stops being reviewable.
            resources: [.copy("Picker.js")]
        ),
        // The fixtures are bytes captured from real `claude` runs — an auth failure as the CLI
        // actually emits it. A fixture written from memory passes while the app fails.
        .testTarget(name: "KeelAppTests", dependencies: ["KeelApp"], path: "Tests/KeelAppTests",
                    resources: [.copy("Fixtures")]),
    ]
)
