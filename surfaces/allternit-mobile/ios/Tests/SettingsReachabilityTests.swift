import XCTest
@testable import Allternit

/// Settings groups (phone calm step 30, brief item 10; web PR #1159
/// `settings.config.test.ts`): the root stays about 17 rows, and moving pages
/// under Advanced never loses one. Also the Open links in rule and the Light
/// default for people with no saved theme.
@MainActor
final class SettingsReachabilityTests: XCTestCase {

    // MARK: - Reachability

    func testEveryPageIsReachableFromTheRootOrAdvanced() {
        var reachable = Set(SettingsRoot.pageIDs())
        for section in SettingsAdvancedSection.allCases {
            reachable.formUnion(SettingsRoot.advancedItems(section))
        }
        let missing = SettingsPageID.allCases.filter { !reachable.contains($0) }
        XCTAssertEqual(missing, [], "Pages with no way in from Settings: \(missing.map(\.rawValue))")
    }

    func testNoPageIsListedTwice() {
        var seen: [SettingsPageID] = SettingsRoot.pageIDs()
        for section in SettingsAdvancedSection.allCases {
            seen += SettingsRoot.advancedItems(section)
        }
        XCTAssertEqual(seen.count, Set(seen).count, "A page shows on the root and on Advanced, or twice on one")
        XCTAssertEqual(seen.count, SettingsPageID.allCases.count)
    }

    func testAdvancedRowCountsEveryPageNotOnTheRoot() {
        XCTAssertEqual(SettingsRoot.advancedPageCount(),
                       SettingsPageID.allCases.count - SettingsRoot.pageIDs().count)
    }

    // MARK: - Root shape

    func testRootHasAboutSeventeenRows() {
        let rows = SettingsRoot.rowCount()
        XCTAssertTrue((15...19).contains(rows), "Settings root has \(rows) rows; the brief asks for about 17")
    }

    func testRootGroupsAndOrderMatchTheBrief() {
        let groups: [(String, [String])] = SettingsRoot.entries.compactMap { entry in
            guard case .group(let label, let rows) = entry else { return nil }
            return (label, rows.map(\.title))
        }
        XCTAssertEqual(groups.map(\.0), ["Account", "Assistant", "Bots and computers", "App"])
        XCTAssertEqual(groups[0].1, ["Profile", "Billing", "Usage", "Notifications", "Privacy"])
        XCTAssertEqual(groups[1].1, ["Catalog", "Memory", "Voice and dictation"])
        XCTAssertEqual(groups[2].1, ["Connections", "Computers and devices", "Agent rules"])
        XCTAssertEqual(groups[3].1, ["Open links in", "Haptic feedback", "Reduce motion"])
    }

    func testRootStartsWithAccountsAndEndsWithAdvancedThenLogOut() {
        guard case .accounts = SettingsRoot.entries.first else { return XCTFail("Accounts card is not first") }
        let tail = SettingsRoot.entries.suffix(3)
        guard tail.count == 3 else { return XCTFail("Root too short") }
        let kinds = tail.map { entry -> String in
            switch entry {
            case .appearance: return "appearance"
            case .advanced: return "advanced"
            case .logout: return "logout"
            default: return "other"
            }
        }
        XCTAssertEqual(kinds, ["appearance", "advanced", "logout"])
    }

    func testRootRenamesOnlyChangeThePhoneLabel() {
        XCTAssertEqual(SettingsRoot.label(for: .voice), "Voice and dictation")
        XCTAssertEqual(SettingsRoot.label(for: .computers), "Computers and devices")
        XCTAssertEqual(SettingsRoot.label(for: .skills), "Skills")
        XCTAssertEqual(SettingsPageID.voice.label, "Voice")
    }

    func testAdvancedSectionsFollowTheWebOrder() {
        XCTAssertEqual(SettingsAdvancedSection.allCases.map(\.label),
                       ["Account", "Assistant", "Bots and agents", "Computers and devices", "Developer", "Operator", "About"])
        XCTAssertEqual(SettingsRoot.advancedItems(.account), [.signin, .organization, .setup])
        XCTAssertEqual(SettingsRoot.advancedItems(.about), [.about])
    }

    // MARK: - Open links in

    func testInAppOpensOrdinaryWebPagesInTheApp() {
        let url = URL(string: "https://allternit.com/terms")!
        XCTAssertEqual(openLinkTarget(for: url, preference: .inApp), .inApp)
        XCTAssertEqual(openLinkTarget(for: url, preference: .safari), .system)
    }

    func testNonWebLinksAlwaysGoToTheSystem() {
        for raw in ["app-settings:", "mailto:privacy@allternit.com", "allternit://inbox/1", "tel:5551234"] {
            XCTAssertEqual(openLinkTarget(for: URL(string: raw)!, preference: .inApp), .system, raw)
        }
    }

    func testAppStoreAndPurchaseLinksLeaveTheApp() {
        for raw in [
            "https://apps.apple.com/app/id6828841999?action=write-review",
            "https://allternit.com/pricing",
            "https://ai.allternit.com/settings/billing",
            "https://checkout.stripe.com/c/pay/cs_test",
            "https://billing.stripe.com/p/session/abc",
        ] {
            XCTAssertEqual(openLinkTarget(for: URL(string: raw)!, preference: .inApp), .system, raw)
        }
        // Not a purchase page just because the path starts with the same letters.
        XCTAssertEqual(openLinkTarget(for: URL(string: "https://allternit.com/pricingfaq")!, preference: .inApp), .inApp)
    }

    // MARK: - Light default

    func testNoSavedThemeMeansLight() {
        XCTAssertEqual(AppearanceStore.Mode.initial(saved: nil), .light)
        XCTAssertEqual(AppearanceStore.Mode.initial(saved: "nonsense"), .light)
    }

    func testASavedThemeIsKept() {
        XCTAssertEqual(AppearanceStore.Mode.initial(saved: "system"), .system)
        XCTAssertEqual(AppearanceStore.Mode.initial(saved: "dark"), .dark)
    }
}
