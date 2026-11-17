import CoreGraphics
import Foundation

/// Pure rules for the folded composer deck band (phone calm pass, contract
/// C1). Mirrors the web's `components/composer-deck/deck-open.ts`.

/// Upward travel (pt) on the composer that opens the folded band.
let deckSwipeDistance: CGFloat = 24

/// The band is folded at rest and open when any condition holds: focus is in
/// the composer, deck or their menus; the draft has text or attachments; the
/// person swiped up and has not swiped down; a deck mode is armed (D6).
func deckOpen(focused: Bool, hasDraft: Bool, swipedOpen: Bool, modeArmed: Bool) -> Bool {
    focused || hasDraft || swipedOpen || modeArmed
}

/// A vertical swipe's next `swipedOpen`: up by 24 pt or more opens, down by
/// 24 pt or more closes, anything shorter leaves it as it was.
func nextSwipedOpen(current: Bool, startY: CGFloat, endY: CGFloat) -> Bool {
    let dy = startY - endY
    if dy >= deckSwipeDistance { return true }
    if dy <= -deckSwipeDistance { return false }
    return current
}

/// Once-card bookkeeping for the swipe tip (web `COMPOSER_SWIPE_NUDGE_TEXT`).
enum ComposerSwipeTipState {
    static let text = "Swipe the composer up for modes, plugins and artifacts"
    static let defaultsKey = "allternit.nudge.composer-swipe"

    /// Shows while unseen, with the deck folded, and never beside the computer
    /// chip (one card at a time).
    static func shouldShow(seen: Bool, deckIsOpen: Bool, chipVisible: Bool, hasDeck: Bool) -> Bool {
        hasDeck && !seen && !deckIsOpen && !chipVisible
    }
}
