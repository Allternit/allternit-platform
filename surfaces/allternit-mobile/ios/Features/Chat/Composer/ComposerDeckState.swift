import CoreGraphics
import Foundation

/// Pure rules for the folded composer deck band (phone calm pass, contract
/// C1). Mirrors the web's `components/composer-deck/deck-open.ts`.

/// Upward travel (pt) on the composer that opens the folded band.
let deckSwipeDistance: CGFloat = 24

/// UserDefaults key for the person's choice (per device, Eoj 2026-11-18).
let deckFoldedDefaultsKey = "allternit.composer.deckFolded"

/// The band is OPEN at rest. Only the "tools" tab (or a swipe) folds it, and
/// the choice persists. Focus, typing and armed modes never change it.
func deckOpen(folded: Bool) -> Bool {
    !folded
}

/// A vertical swipe's next `swipedOpen` (true = open): up by 24 pt or more opens,
/// down by 24 pt or more folds, anything shorter leaves it as it was.
func nextSwipedOpen(current: Bool, startY: CGFloat, endY: CGFloat) -> Bool {
    let dy = startY - endY
    if dy >= deckSwipeDistance { return true }
    if dy <= -deckSwipeDistance { return false }
    return current
}
