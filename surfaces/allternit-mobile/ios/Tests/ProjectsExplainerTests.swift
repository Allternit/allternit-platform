import XCTest
@testable import Allternit

final class ProjectsExplainerTests: XCTestCase {
    func testOnceKeyAndCopyMatchWeb() {
        XCTAssertEqual(ProjectsExplainer.onceKey, "allternit.nudge.projects-explainer")
        XCTAssertEqual(ProjectsExplainer.title, "Every project is a folder your team works in")
    }

    func testSeenFlagPersists() {
        let defaults = UserDefaults(suiteName: "ProjectsExplainerTests")!
        defaults.removePersistentDomain(forName: "ProjectsExplainerTests")
        XCTAssertFalse(ProjectsExplainer.seen(defaults))
        ProjectsExplainer.markSeen(defaults)
        XCTAssertTrue(ProjectsExplainer.seen(defaults))
    }
}
