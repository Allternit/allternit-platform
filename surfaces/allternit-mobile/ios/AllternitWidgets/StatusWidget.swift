import AppIntents
import SwiftUI
import WidgetKit

// ─────────────────────────────────────────────────────────────────────────────
// The one flipping Allternit widget (plan I6a spike + I6). Front = status,
// back = actions (Eoj's pick; the front face is configurable). A round arrow
// flips it; it turns back to the front after 10 minutes.
//
// The four device questions the spike must answer (none is verified yet), and
// how this code behaves whichever way each one comes out:
//
// 1. Does WidgetKit play a custom 3D transition between entries?
//    The face view is keyed by `.id(face)` and carries `FlipTransition`
//    (rotation3DEffect around Y). If the phone shows a hard cut instead, set
//    `StatusWidgetTuning.transition = .push`: the same views then use
//    `.push(from: .trailing)` plus scale. Reduce Motion always uses `.opacity`.
//
// 2. Does `openAppWhenRun` work from a small widget on iOS 17?
//    Small-size action buttons are `Button(intent: OpenAllternitWidgetLinkIntent)`.
//    If the app opens, the intent also leaves the URL in
//    `WidgetFlags.pendingLink`, so the app can route it on foreground. If it
//    does not open the app, switch the small actions face to `.widgetURL`
//    for one action (Needs you) and keep the rest in the medium size.
//    iOS 18+ uses `OpenAllternitURLIntent` (system `OpenURLIntent`).
//
// 3. Which process runs an intent compiled into both targets?
//    `FlipWidgetIntent` only touches the App Group and calls
//    `WidgetCenter.reloadTimelines`, so it is correct in either process.
//    `OpenAllternitWidgetLinkIntent` (openAppWhenRun) runs in the app; the app hands
//    the URL to the router through `WidgetLinkOpener.handler` and/or the
//    pending link. If the extension runs it instead, only the pending link
//    is written and the app picks it up on the next foreground.
//
// 4. Is `accessibilityReduceMotion` set in the widget environment?
//    The view reads both the environment value and the App Group flag
//    `WidgetFlags.reduceMotion` (written by `StatusSnapshotWriter`), and
//    reduces motion if either is true. If the environment value is always
//    false on device, the flag alone carries the setting.
// ─────────────────────────────────────────────────────────────────────────────

enum StatusWidgetTuning {
    enum Transition: Sendable { case flip3D, push }
    /// Flip to `.push` if device question 1 says the 3D transition is cut.
    static let transition: Transition = .flip3D
    /// The stored snapshot is refetched when older than this.
    static let staleAfter: TimeInterval = 15 * 60
}

// MARK: Configuration

/// Seam for the entity types that `SystemSurfaces/Intents/Entities.swift` will
/// define. Swapping is two lines: point these typealiases at the `AppEntity`
/// types, and change the bodies of `WidgetConfigSeam.botID` and `.routineID`
/// to return `entity.id`. Nothing else here reads the parameters.
typealias WidgetBotParam = BotEntity
typealias WidgetRoutineParam = RoutineEntity

enum WidgetConfigSeam {
    static func botID(_ value: WidgetBotParam?) -> String? {
        guard let v = value?.id.trimmingCharacters(in: .whitespacesAndNewlines), !v.isEmpty else { return nil }
        return v
    }
    static func routineID(_ value: WidgetRoutineParam?) -> String? {
        guard let v = value?.id.trimmingCharacters(in: .whitespacesAndNewlines), !v.isEmpty else { return nil }
        return v
    }
}

enum WidgetFrontFace: String, AppEnum {
    case status
    case actions

    static var typeDisplayRepresentation: TypeDisplayRepresentation { "Front face" }
    static var caseDisplayRepresentations: [WidgetFrontFace: DisplayRepresentation] {
        [.status: "Status", .actions: "Actions"]
    }
    var face: WidgetFace { self == .status ? .status : .actions }
}

struct StatusWidgetIntent: WidgetConfigurationIntent {
    static var title: LocalizedStringResource { "Allternit" }
    static var description: IntentDescription { IntentDescription("Status on the front, actions on the back.") }

    @Parameter(title: "Front face", default: .status) var frontFace: WidgetFrontFace
    @Parameter(title: "Favourite routine") var routine: WidgetRoutineParam?
    @Parameter(title: "Bot for Ask") var bot: WidgetBotParam?
}

// MARK: Entry

enum StatusContent: Sendable, Equatable {
    case signedOut
    case noComputer
    case paused(StatusSnapshot)
    case quiet(StatusSnapshot)
    case live(StatusSnapshot)

    var snapshot: StatusSnapshot? {
        switch self {
        case .paused(let s), .quiet(let s), .live(let s): return s
        case .signedOut, .noComputer: return nil
        }
    }
}

struct StatusEntry: TimelineEntry {
    var date: Date
    var face: WidgetFace
    var front: WidgetFace
    var slot: String
    var content: StatusContent
    var botID: String?
    var routineID: String?
}

// MARK: Provider

struct StatusProvider: AppIntentTimelineProvider {
    typealias Entry = StatusEntry
    typealias Intent = StatusWidgetIntent

    func placeholder(in context: Context) -> StatusEntry {
        var snap = StatusSnapshot()
        snap.activeBots = 2
        snap.pendingApprovals = 1
        snap.needsYou = ["placeholder"]
        var focus = StatusSnapshot.Focus()
        focus.botName = "Raven"
        focus.step = "Drafting the reply"
        snap.focus = focus
        return StatusEntry(date: Date(), face: .status, front: .status,
                           slot: "placeholder", content: .live(snap), botID: nil, routineID: nil)
    }

    func snapshot(for configuration: StatusWidgetIntent, in context: Context) async -> StatusEntry {
        if context.isPreview { return placeholder(in: context) }
        return await makeEntries(for: configuration, family: context.family, fetch: false).first
            ?? placeholder(in: context)
    }

    func timeline(for configuration: StatusWidgetIntent, in context: Context) async -> Timeline<StatusEntry> {
        let entries = await makeEntries(for: configuration, family: context.family, fetch: true)
        // The last entry is either the current one or the return-to-front one;
        // refresh at the stale limit after the first entry either way.
        let refresh = (entries.first?.date ?? Date()).addingTimeInterval(StatusWidgetTuning.staleAfter)
        return Timeline(entries: entries, policy: .after(refresh))
    }

    private func makeEntries(for config: StatusWidgetIntent, family: WidgetFamily, fetch: Bool) async -> [StatusEntry] {
        let now = Date()
        let front = config.frontFace.face
        let slot = WidgetFlip.slot(family: Self.familyName(family), front: front)
        let content = await Self.resolveContent(fetch: fetch, now: now)
        let botID = WidgetConfigSeam.botID(config.bot)
        let routineID = WidgetConfigSeam.routineID(config.routine)

        func entry(_ date: Date, _ face: WidgetFace) -> StatusEntry {
            StatusEntry(date: date, face: face, front: front, slot: slot, content: content,
                        botID: botID, routineID: routineID)
        }

        // Accessory families are status only and never flip.
        if Self.isAccessory(family) { return [entry(now, .status)] }

        if WidgetFlip.isFlipped(slot: slot, now: now), let at = AppGroupStore.flippedAt(slot: slot) {
            return [entry(now, front.other),
                    entry(at.addingTimeInterval(WidgetFlip.returnAfter), front)]
        }
        return [entry(now, front)]
    }

    // MARK: Data

    /// The App Group snapshot first; refetch when older than 15 minutes and a
    /// read token exists.
    static func resolveContent(fetch: Bool, now: Date) async -> StatusContent {
        let token = SharedAccess.readToken()
        guard AppGroupStore.signedIn || token != nil else { return .signedOut }

        var snap = AppGroupStore.snapshot
        let stale = snap.map { now.timeIntervalSince($0.asOf ?? .distantPast) > StatusWidgetTuning.staleAfter } ?? true
        if fetch, stale, token != nil {
            switch await StatusSummaryClient().fetch() {
            case .success(let fresh):
                AppGroupStore.saveSnapshot(fresh, now: now)
                snap = AppGroupStore.snapshot ?? fresh
            case .failure(.signedOut):
                if snap == nil { return .signedOut }
            case .failure:
                break // keep whatever the app last stored
            }
        }
        guard let snap else {
            return AppGroupStore.hasComputer ? .quiet(StatusSnapshot()) : .noComputer
        }
        if snap.paused { return .paused(snap) }
        let needs = max(snap.needsYou.count, snap.pendingApprovals)
        if snap.activeBots == 0, needs == 0, snap.attentionBots == 0, snap.focus == nil { return .quiet(snap) }
        return .live(snap)
    }

    static func isAccessory(_ family: WidgetFamily) -> Bool {
        family == .accessoryRectangular || family == .accessoryCircular || family == .accessoryInline
    }

    static func familyName(_ family: WidgetFamily) -> String {
        switch family {
        case .systemSmall: return "small"
        case .systemMedium: return "medium"
        case .accessoryRectangular: return "accessoryRectangular"
        case .accessoryCircular: return "accessoryCircular"
        default: return "other"
        }
    }
}

// MARK: Widget

struct StatusWidget: Widget {
    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: StatusWidgetKind.id, intent: StatusWidgetIntent.self, provider: StatusProvider()) { entry in
            StatusWidgetView(entry: entry)
        }
        .configurationDisplayName("Allternit")
        .description("Status on the front, actions on the back. Tap the arrow to flip.")
        .supportedFamilies([.systemSmall, .systemMedium, .accessoryRectangular, .accessoryCircular])
    }
}

// MARK: iOS 26 widget push

/// Receives the widget push token (iOS 26; confirmed in the WidgetKit
/// swiftinterface: `WidgetPushHandler.pushTokenDidChange(_:widgets:)`) and
/// leaves it in the App Group (`WidgetPushToken.key`); the app uploads it as
/// `kind: "widget"` on the next foreground (`ActivityTokenRegistry.syncWidgetToken`).
/// Attached through `StatusWidgetWithPush`, which the iOS 26 bundle uses.
@available(iOS 26.0, *)
struct StatusWidgetPushHandler: WidgetPushHandler {
    init() {}
    func pushTokenDidChange(_ pushInfo: WidgetPushInfo, widgets: [WidgetInfo]) {
        let hex = pushInfo.token.map { String(format: "%02x", $0) }.joined()
        AppGroup.defaults?.set(hex, forKey: WidgetPushToken.key)
    }
}

/// `StatusWidget` with the iOS 26 push handler: same kind, same faces.
@available(iOS 26.0, *)
struct StatusWidgetWithPush: Widget {
    var body: some WidgetConfiguration {
        StatusWidget().body.pushHandler(StatusWidgetPushHandler.self)
    }
}
