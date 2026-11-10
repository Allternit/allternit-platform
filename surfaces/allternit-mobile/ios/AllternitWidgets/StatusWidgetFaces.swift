import AppIntents
import SwiftUI
import WidgetKit

// Faces for the flipping widget. Colours come from the asset catalog
// (BgPanel, TextPrimary, TextSecondary, FillNeutral, BrandAccent); no tan
// surfaces. Amber marks "needs you", the same as the Live Activity mockups.

private enum Palette {
    static let panel = Color("BgPanel")
    static let primary = Color("TextPrimary")
    static let secondary = Color("TextSecondary")
    static let fill = Color("FillNeutral")
    static let working = Color("BrandAccent")
    static let amber = Color(red: 0.89, green: 0.67, blue: 0.27)
}

// MARK: Transition

/// Y-axis flip for the face change. Hidden while the face is edge-on.
struct FlipTransition: Transition {
    func body(content: Content, phase: TransitionPhase) -> some View {
        content
            .rotation3DEffect(
                .degrees(phase.isIdentity ? 0 : (phase == .willAppear ? -90 : 90)),
                axis: (x: 0, y: 1, z: 0),
                perspective: 0.6
            )
            .opacity(phase.isIdentity ? 1 : 0)
    }
}

// MARK: Root

struct StatusWidgetView: View {
    let entry: StatusEntry
    @Environment(\.widgetFamily) private var family
    @Environment(\.accessibilityReduceMotion) private var reduceMotionEnv

    private var reduceMotion: Bool { reduceMotionEnv || WidgetFlags.reduceMotion }

    var body: some View {
        switch family {
        case .accessoryRectangular:
            AccessoryRectangularView(entry: entry)
        case .accessoryCircular:
            AccessoryCircularView(entry: entry)
        default:
            ZStack {
                if entry.face == .status {
                    StatusFace(entry: entry, compact: family == .systemSmall)
                        .id(WidgetFace.status)
                        .transition(faceTransition)
                } else {
                    ActionsFace(entry: entry, compact: family == .systemSmall)
                        .id(WidgetFace.actions)
                        .transition(faceTransition)
                }
            }
            .overlay(alignment: .topTrailing) { FlipButton(slot: entry.slot) }
            .containerBackground(Palette.panel, for: .widget)
        }
    }

    private var faceTransition: AnyTransition {
        if reduceMotion { return .opacity }
        switch StatusWidgetTuning.transition {
        case .flip3D: return AnyTransition(FlipTransition())
        case .push: return AnyTransition.push(from: .trailing).combined(with: .scale(scale: 0.92))
        }
    }
}

private struct FlipButton: View {
    let slot: String
    var body: some View {
        Button(intent: FlipWidgetIntent(slot: slot)) {
            Image(systemName: "arrow.triangle.2.circlepath")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Palette.secondary)
                .frame(width: 24, height: 24)
                .background(Circle().fill(Palette.fill))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Flip widget")
    }
}

// MARK: Copy

private enum Copy {
    static func needsCount(_ s: StatusSnapshot) -> Int { max(s.needsYou.count, s.pendingApprovals) }

    static func routineLine(_ r: StatusSnapshot.LastRoutine) -> String {
        let word: String
        switch r.status.lowercased() {
        case "ok", "success", "succeeded", "done", "completed": word = "done"
        case "failed", "error": word = "failed"
        case "running": word = "running"
        default: word = r.status
        }
        return r.title.isEmpty ? word : "\(r.title) \(word)"
    }

    /// Title and detail for the four non-live states.
    static func state(_ content: StatusContent) -> (title: String, detail: String)? {
        switch content {
        case .signedOut: return ("Sign in to Allternit", "Open the app to sign in.")
        case .noComputer: return ("Pair a computer", "Your bots run on a computer.")
        case .paused: return ("Bots paused", "Resume them in the app.")
        case .quiet: return ("All quiet", "Nothing needs you.")
        case .live: return nil
        }
    }
}

// MARK: Status face

struct StatusFace: View {
    let entry: StatusEntry
    let compact: Bool

    var body: some View {
        if let state = Copy.state(entry.content) {
            VStack(alignment: .leading, spacing: 4) {
                Text("A://TERNIT").font(.system(size: 9, weight: .bold, design: .monospaced))
                    .foregroundStyle(Palette.secondary)
                Spacer(minLength: 0)
                Text(state.title).font(.system(compact ? .headline : .title3, weight: .semibold))
                    .foregroundStyle(Palette.primary).lineLimit(2).minimumScaleFactor(0.8)
                Text(state.detail).font(.caption).foregroundStyle(Palette.secondary).lineLimit(2)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        } else if let snap = entry.content.snapshot {
            live(snap)
        }
    }

    private func live(_ snap: StatusSnapshot) -> some View {
        let needs = Copy.needsCount(snap)
        return VStack(alignment: .leading, spacing: compact ? 6 : 8) {
            HStack(alignment: .firstTextBaseline, spacing: compact ? 10 : 18) {
                count(snap.activeBots, "working", Palette.working)
                count(needs, "needs you", needs > 0 ? Palette.amber : Palette.secondary)
            }
            Spacer(minLength: 0)
            if let focus = snap.focus, !focus.botName.isEmpty {
                VStack(alignment: .leading, spacing: 1) {
                    Text(focus.botName).font(.system(.footnote, weight: .semibold))
                        .foregroundStyle(Palette.primary).lineLimit(1)
                    if !focus.step.isEmpty {
                        Text(focus.step).font(.caption2).foregroundStyle(Palette.secondary).lineLimit(compact ? 1 : 2)
                    }
                }
            }
            if !compact, let r = snap.lastRoutine {
                Text(Copy.routineLine(r)).font(.caption2).foregroundStyle(Palette.secondary).lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
    }

    private func count(_ n: Int, _ label: String, _ color: Color) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("\(n)").font(.system(size: compact ? 30 : 36, weight: .semibold, design: .rounded))
                .foregroundStyle(color).contentTransition(.numericText())
            Text(label).font(.caption2).foregroundStyle(Palette.secondary)
        }
    }
}

// MARK: Actions face

struct ActionsFace: View {
    let entry: StatusEntry
    let compact: Bool

    private var askURL: String {
        if let id = entry.botID ?? entry.content.snapshot?.focus?.botId, !id.isEmpty {
            return "allternit://bot/\(id)/chat"
        }
        return "allternit://call/main"
    }

    private var routineURL: String? {
        guard let id = entry.routineID else { return nil }
        return "allternit://routine/\(id)"
    }

    var body: some View {
        let needs = entry.content.snapshot.map(Copy.needsCount) ?? 0
        let items: [ActionItem] = [
            ActionItem(title: "Ask a bot", icon: "bubble.left.fill", url: askURL, tint: Palette.primary),
            ActionItem(title: "Call main bot", icon: "phone.fill", url: "allternit://call/main", tint: Palette.primary),
            ActionItem(title: needs > 0 ? "Needs you \(needs)" : "Needs you", icon: "tray.fill",
                       url: "allternit://inbox", tint: needs > 0 ? Palette.amber : Palette.primary),
            ActionItem(title: "Run routine", icon: "play.fill",
                       url: routineURL ?? "allternit://inbox", tint: Palette.primary,
                       enabled: routineURL != nil),
        ]
        if compact {
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 6), GridItem(.flexible(), spacing: 6)], spacing: 6) {
                ForEach(items) { ActionButton(item: $0, compact: true) }
            }
            .padding(.top, 20)
        } else {
            HStack(spacing: 8) {
                ForEach(items) { ActionButton(item: $0, compact: false) }
            }
            .padding(.top, 20)
        }
    }
}

private struct ActionItem: Identifiable {
    var title: String
    var icon: String
    var url: String
    var tint: Color
    var enabled: Bool = true
    var id: String { title }
}

private struct ActionButton: View {
    let item: ActionItem
    let compact: Bool

    var body: some View {
        Group {
            if #available(iOS 18.0, *) {
                Button(intent: OpenAllternitURLIntent(urlString: item.url)) { label }
            } else {
                Button(intent: OpenAllternitWidgetLinkIntent(urlString: item.url)) { label }
            }
        }
        .buttonStyle(.plain)
        .opacity(item.enabled ? 1 : 0.4)
        .disabled(!item.enabled)
    }

    private var label: some View {
        VStack(spacing: 3) {
            Image(systemName: item.icon).font(.system(size: compact ? 15 : 18, weight: .semibold))
                .foregroundStyle(item.tint)
            Text(item.title).font(.system(size: compact ? 9 : 10, weight: .semibold))
                .foregroundStyle(Palette.primary).lineLimit(1).minimumScaleFactor(0.7)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.vertical, 4)
        .background(RoundedRectangle(cornerRadius: 11, style: .continuous).fill(Palette.fill))
    }
}

// MARK: Lock Screen (status only, no flip)

struct AccessoryRectangularView: View {
    let entry: StatusEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            if let state = Copy.state(entry.content) {
                Text(state.title).font(.headline).lineLimit(1)
                Text(state.detail).font(.caption2).lineLimit(1)
            } else if let snap = entry.content.snapshot {
                let needs = Copy.needsCount(snap)
                Text("\(snap.activeBots) working, \(needs) needs you").font(.headline).lineLimit(1)
                if let focus = snap.focus, !focus.botName.isEmpty {
                    Text(focus.step.isEmpty ? focus.botName : "\(focus.botName): \(focus.step)")
                        .font(.caption2).lineLimit(1)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .containerBackground(.clear, for: .widget)
    }
}

struct AccessoryCircularView: View {
    let entry: StatusEntry

    var body: some View {
        ZStack {
            AccessoryWidgetBackground()
            if let snap = entry.content.snapshot, entry.content != .signedOut {
                let needs = Copy.needsCount(snap)
                VStack(spacing: 0) {
                    Text("\(needs > 0 ? needs : snap.activeBots)").font(.system(.title3, design: .rounded, weight: .semibold))
                    Text(needs > 0 ? "need" : "work").font(.system(size: 8, weight: .medium))
                }
            } else {
                Image(systemName: "person.crop.circle.badge.exclamationmark")
            }
        }
        .containerBackground(.clear, for: .widget)
    }
}
