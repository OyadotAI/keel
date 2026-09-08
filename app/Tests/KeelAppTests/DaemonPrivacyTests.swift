import XCTest
@testable import KeelApp

final class DaemonPrivacyTests: XCTestCase {
    @MainActor
    func testConfiguredReportingKeyDoesNotEnableAnIndependentDaemonReporter() {
        let arguments = Daemon.launchArguments(port: 7788, resumeLast: true,
            sentryDSN: "https://fixture.invalid/reporting")
        XCTAssertFalse(arguments.contains("--sentry-dsn"))
        XCTAssertFalse(arguments.contains("https://fixture.invalid/reporting"))
        XCTAssertTrue(arguments.contains("--exit-with-parent"))
        XCTAssertTrue(arguments.contains("--resume-last"))
    }

    @MainActor
    func testDetachedDaemonKeepsItsPortAndDoesNotResumeAnotherProject() {
        XCTAssertEqual(Daemon.launchArguments(port: 7789, resumeLast: false, sentryDSN: nil),
                       ["serve", "--port", "7789", "--exit-with-parent"])
    }
}
