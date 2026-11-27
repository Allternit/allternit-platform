import SwiftUI

/// Chat wait, design pick B: "your message anchored to the top". When the
/// person sends, the thread scrolls once so their message sits at the top of
/// the view, and room is kept under it (one view tall, minus what is already
/// below the message). The answer, a picture card and its result grow downward
/// into that room: the room shrinks by what they add, so nothing on screen
/// moves. While a turn is anchored, the chat does not follow the bottom; the
/// jump button takes you down when the answer outgrows the view. Live status
/// is not added inline: it stays in the running strip above the composer.
/// Mirrors allternit-ai `useAnswerRoom`. Reduce Motion: the one scroll jumps.
enum AnswerRoom {
    /// Space kept above the anchored message.
    static let topGap: CGFloat = 16
    /// The thread's own row spacing (LazyVStack), counted between the turn and the room.
    static let rowSpacing: CGFloat = 20

    /// The message to anchor when a turn starts running: the newest one the
    /// person sent. Nil when no turn just started or there is no such message.
    static func anchorId(wasStreaming: Bool, isStreaming: Bool, messages: [MessageRecord]) -> String? {
        guard isStreaming, !wasStreaming else { return nil }
        return messages.last(where: { $0.role == "user" })?.id
    }

    /// Height of the spacer under the anchored turn: what is missing to fill one view.
    static func room(viewport: CGFloat, turnHeight: CGFloat) -> CGFloat {
        max(0, (viewport - turnHeight - topGap - rowSpacing).rounded(.up))
    }

    /// An anchored turn that is still running must not drag the view to the bottom.
    static func followsBottom(anchored: Bool, streaming: Bool) -> Bool {
        !(anchored && streaming)
    }

    /// Where the anchored turn starts in the thread; nil when it is not there (another chat).
    static func split(_ messages: [MessageRecord], anchor: String?) -> (head: ArraySlice<MessageRecord>, turn: ArraySlice<MessageRecord>)? {
        guard let anchor, let index = messages.firstIndex(where: { $0.id == anchor }) else { return nil }
        return (messages[..<index], messages[index...])
    }
}

/// Whether a thread follows new text: only while the reader is at the bottom,
/// and only the reader's own drag up releases it. Programmatic scrolls, growth
/// and layout never release it. While released, new text sets `hasNew` so the
/// "New text" pill can show; reaching the bottom, jumping or sending re-follows.
struct TailFollow: Equatable {
    var follows = true
    var hasNew = false

    /// The reader dragged toward earlier messages.
    mutating func userScrolledUp() { follows = false }
    /// The end of the thread is back in view.
    mutating func reachedBottom() { follows = true; hasNew = false }
    /// New text arrived. Counts as unseen only when not following.
    mutating func grew() { if !follows { hasNew = true } }
    /// The reader tapped the jump button.
    mutating func jumped() { follows = true; hasNew = false }
    /// The reader sent a message.
    mutating func sent() { follows = true; hasNew = false }
}

/// The anchored turn's height and the scroll view's height, read by two backgrounds.
struct AnswerRoomTurnHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

struct AnswerRoomViewportKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}
