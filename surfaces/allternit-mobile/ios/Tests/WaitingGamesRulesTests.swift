import XCTest
@testable import Allternit

/// Big Fish's rules: the same as the web's waiting-games-engine.ts.
final class WaitingGamesRulesTests: XCTestCase {
    /// The bug Eoj hit: at the old start size nothing was smaller, so nothing was edible.
    func testEveryPlayerSizeHasFishToEat() {
        for size in FishRules.startSize...FishRules.maxSize {
            for step in 0..<100 {
                let r2 = Double(step) / 100
                let small = FishRules.spawnSize(playerSize: size, r1: 0.9, r2: r2)
                XCTAssertTrue((1..<size).contains(small), "size \(size) spawned \(small)")
            }
        }
    }

    func testBiggerFishAreBiggerButNeverPastTheTopSize() {
        for size in FishRules.startSize..<FishRules.maxSize {
            let big = FishRules.spawnSize(playerSize: size, r1: 0.1, r2: 0.99)
            XCTAssertGreaterThan(big, size)
            XCTAssertLessThanOrEqual(big, FishRules.maxSize)
        }
        XCTAssertLessThan(FishRules.spawnSize(playerSize: FishRules.maxSize, r1: 0.1, r2: 0.5), FishRules.maxSize)
    }

    func testEatingIsMoreForgivingThanBeingEaten() {
        // Near miss: within a bite, but no threat at that distance.
        let gap = (x: 2.0, y: 1.2)
        XCTAssertTrue(FishRules.bites(gapX: gap.x, gapY: gap.y, widths: 11, heights: 2))
        XCTAssertFalse(FishRules.caught(gapX: gap.x, gapY: gap.y, widths: 11, heights: 2))
    }
}
