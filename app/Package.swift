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
    ],
    targets: [
        .executableTarget(
            name: "KeelApp",
            dependencies: [.product(name: "SwiftTerm", package: "SwiftTerm")],
            path: "Sources/KeelApp",
            // The picker is JavaScript that runs inside someone else's page, so it ships as a
            // resource rather than a Swift string literal — escaping a hundred lines of JS into
            // source is how it stops being reviewable.
            resources: [.copy("Picker.js")]
        ),
        .testTarget(name: "KeelAppTests", dependencies: ["KeelApp"], path: "Tests/KeelAppTests"),
    ]
)
