import SwiftUI
import UIKit

// The in-app island bar (plan D15, mockup handoffs/island-bar-team-map-2026-11-25/
// mockups/B-ios-island.jsx). Apps cannot draw in the real Dynamic Island, so
// this is our own black capsule that grows out of the island's spot: width
// first, then height, from the top centre (spring response 0.35, damping
// 0.85). On phones with no island a small "A://" capsule sits just under
// the status bar and grows the same way. Reduce Motion: no grow, the bar
// fades in at its final size (0.15 s). Outside the app, the job's Live
// Activity (TaskLiveActivity) uses the real island.

/// Sizes and timings from the mockup's beat sheet.
enum IslandBarMetrics {
    /// The real island's footprint (iPhone 14 Pro and later).
    static let islandSize = CGSize(width: 126, height: 37)
    static let islandTop: CGFloat = 11
    /// The handle on phones with no island.
    static let handleSize = CGSize(width: 96, height: 26)
    /// The live island around the cutout (compact Live Activity width).
    static let liveWidth: CGFloat = 236
    static let barHeight: CGFloat = 60
    static let grow: Animation = .spring(response: 0.35, dampingFraction: 0.85)
    static let reducedFade: Animation = .easeInOut(duration: 0.15)
    static let ink = Color(red: 0.043, green: 0.043, blue: 0.043)
    static let chipText = Color(red: 0.925, green: 0.925, blue: 0.922)
    static let mute = Color(red: 0.608, green: 0.604, blue: 0.592)

    /// A Dynamic Island phone in portrait has a top inset of 51 pt or more
    /// (59 on 14 Pro-16 Pro); notch phones 44-50, Home-button phones 20.
    static func hasIsland(topInset: CGFloat) -> Bool {
        UIDevice.current.userInterfaceIdiom == .phone && topInset >= 51
    }
}

struct IslandBarOverlay: View {
    @ObservedObject var model: IslandBarModel
    let topInset: CGFloat
    let width: CGFloat
    let showsPill: Bool
    @ReducedMotion private var reduceMotion

    private var island: Bool { IslandBarMetrics.hasIsland(topInset: topInset) }
    private var barTop: CGFloat { island ? IslandBarMetrics.islandTop : max(topInset, 20) + 2 }
    private var barWidth: CGFloat { min(width - 24, 380) }
    private var hasLiveJob: Bool { model.jobId != nil && !model.isOpen && (model.step == .live || model.step == .done) }

    var body: some View {
        ZStack(alignment: .top) {
            if model.isOpen {
                Color.black.opacity(0.18)
                    .background(.ultraThinMaterial.opacity(0.6))
                    .ignoresSafeArea()
                    .onTapGesture { toggle(false) }
                    .transition(.opacity)
                    .accessibilityHidden(true)
            }
            VStack(spacing: 8) {
                capsule
                if model.isOpen {
                    IslandBarCard(model: model, onClose: { toggle(false) })
                        .frame(width: barWidth)
                        .transition(reduceMotion ? .opacity
                                    : .opacity.combined(with: .offset(y: -8)).animation(.easeOut(duration: 0.18).delay(0.12)))
                }
            }
            .padding(.top, barTop)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .ignoresSafeArea(edges: .top)
        .animation(reduceMotion ? IslandBarMetrics.reducedFade : IslandBarMetrics.grow, value: model.isOpen)
        .animation(reduceMotion ? IslandBarMetrics.reducedFade : IslandBarMetrics.grow, value: hasLiveJob)
    }

    // MARK: Capsule

    private var size: CGSize {
        if model.isOpen { return CGSize(width: barWidth, height: IslandBarMetrics.barHeight) }
        if hasLiveJob { return CGSize(width: IslandBarMetrics.liveWidth, height: island ? IslandBarMetrics.islandSize.height : 30) }
        return island ? IslandBarMetrics.islandSize : IslandBarMetrics.handleSize
    }

    @ViewBuilder private var capsule: some View {
        if model.isOpen || hasLiveJob || showsPill {
            let s = size
            ZStack {
                // On an island phone the closed pill is the island itself: no fill.
                RoundedRectangle(cornerRadius: s.height / 2, style: .continuous)
                    .fill(island && !model.isOpen && !hasLiveJob ? Color.clear : IslandBarMetrics.ink)
                    .shadow(color: .black.opacity(model.isOpen ? 0.28 : 0), radius: 13, y: 10)
                content
                    .frame(width: s.width, height: s.height)
                    .clipped()
            }
            .frame(width: s.width, height: s.height)
            // Reduce Motion: no grow. Each size is its own view, so closed
            // and open cross-fade in place (0.15 s) instead of resizing.
            .id(reduceMotion ? "capsule-\(model.isOpen)-\(hasLiveJob)" : "capsule")
            .transition(.opacity)
            .contentShape(RoundedRectangle(cornerRadius: s.height / 2, style: .continuous))
            .onTapGesture { if !model.isOpen { toggle(true) } }
            .gesture(DragGesture(minimumDistance: 12).onEnded { g in
                if g.translation.height > 16, !model.isOpen { toggle(true) }
                if g.translation.height < -16, model.isOpen { toggle(false) }
            })
            .accessibilityElement(children: model.isOpen ? .contain : .ignore)
            .accessibilityLabel(model.isOpen ? "" : (hasLiveJob ? "\(liveTitle), \(model.stateLine)" : "Do with an app"))
            .accessibilityHint(model.isOpen ? "" : "Opens the bar")
            .accessibilityAddTraits(model.isOpen ? [] : .isButton)
            .accessibilityIdentifier("islandBarPill")
        }
    }

    @ViewBuilder private var content: some View {
        if model.isOpen {
            IslandBarSentence(model: model)
                .padding(.horizontal, 14)
                .transition(.opacity.animation(.linear(duration: 0.12).delay(reduceMotion ? 0 : 0.06)))
        } else if hasLiveJob {
            HStack(spacing: 0) {
                IslandStatusIcon(state: model.jobState ?? "working", size: 22)
                Spacer(minLength: island ? IslandBarMetrics.islandSize.width - 20 : 8)
                Text(liveTrailing)
                    .font(.caption2.weight(.semibold))
                    .foregroundColor(.white)
                    .lineLimit(1)
                    .frame(maxWidth: 64, alignment: .trailing)
            }
            .padding(.horizontal, 8)
            .transition(.opacity)
        } else if !island {
            Text("A://")
                .font(.system(size: 10, weight: .bold))
                .tracking(0.3)
                .foregroundColor(IslandBarMetrics.chipText)
        }
    }

    private var liveTitle: String { model.intent?.toolTitle ?? "Job" }

    private var liveTrailing: String {
        var c = TaskActivityAttributes.ContentState(state: model.jobState ?? "working")
        c.approverName = model.approverName
        return c.shortState
    }

    private func toggle(_ open: Bool) {
        Haptics.soft()
        if open { model.open() } else { model.close() }
    }
}

// MARK: - Sentence in the bar

/// "refund [Stripe] order [which?]": text parts, the app chip, blank chips.
struct IslandBarSentence: View {
    @ObservedObject var model: IslandBarModel
    @FocusState private var typing: Bool

    var body: some View {
        HStack(spacing: 6) {
            switch model.step {
            case .apps:
                TextField("", text: $model.query,
                          prompt: Text("What do you want to do?").foregroundColor(IslandBarMetrics.mute))
                    .font(.subheadline)
                    .foregroundColor(IslandBarMetrics.chipText)
                    .tint(.white)
                    .focused($typing)
                    .submitLabel(.go)
                    .onSubmit { if let first = model.matchingApps.first(where: \.isRunnable) { Task { await model.pick(app: first) } } }
                    .onAppear { typing = true }
                    .accessibilityIdentifier("islandBarInput")
            case .tools:
                appChip
                Text("What should it do?").font(.subheadline).foregroundColor(IslandBarMetrics.mute)
                Spacer(minLength: 0)
            case .live, .done:
                IslandStatusIcon(state: model.jobState ?? "working", size: 24)
                VStack(alignment: .leading, spacing: 1) {
                    Text(model.intent?.toolTitle ?? "")
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundColor(.white)
                        .lineLimit(1)
                    Text(model.stateLine)
                        .font(.caption)
                        .foregroundColor(IslandBarMetrics.mute)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            default:
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 3) {
                        appChip
                        ForEach(Array((model.intent?.parts ?? []).enumerated()), id: \.offset) { _, part in
                            switch part {
                            case .text(let t):
                                Text(t.trimmingCharacters(in: .whitespaces))
                                    .font(.subheadline)
                                    .foregroundColor(IslandBarMetrics.chipText)
                            case .blank(let key):
                                blankChip(key)
                            }
                        }
                    }
                }
            }
        }
    }

    private var appChip: some View {
        HStack(spacing: 4) {
            Text(String((model.app?.name ?? "?").prefix(1)).uppercased())
                .font(.system(size: 9, weight: .bold))
                .foregroundColor(.white)
                .frame(width: 14, height: 14)
                .background(RoundedRectangle(cornerRadius: 4).fill(Color.white.opacity(0.25)))
            Text(model.app?.name ?? "")
                .font(.subheadline)
                .foregroundColor(IslandBarMetrics.chipText)
                .lineLimit(1)
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 2)
        .background(Capsule().fill(Color.white.opacity(0.15)))
        .accessibilityLabel("App: \(model.app?.name ?? "")")
    }

    private func blankChip(_ key: String) -> some View {
        let blank = model.intent?.blanks.first { $0.key == key }
        let value = model.values[key]
        let focused = model.focusedBlank == key
        let text = value.map { IslandBarWording.display($0, blank: blank) } ?? (blank?.label.lowercased() ?? key)
        return Button {
            Haptics.selection()
            model.focusedBlank = key
        } label: {
            Text(text)
                .font(.subheadline)
                .foregroundColor(value == nil ? IslandBarMetrics.mute : IslandBarMetrics.chipText)
                .lineLimit(1)
                .padding(.horizontal, 7)
                .padding(.vertical, 2)
                .background(Capsule().fill(value == nil ? Color.clear : Color.white.opacity(0.15)))
                .overlay(Capsule().strokeBorder(focused ? IslandBarMetrics.chipText : Color.white.opacity(value == nil ? 0.25 : 0),
                                                lineWidth: focused ? 1.5 : 1))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(blank?.label ?? key): \(value.map { IslandBarWording.display($0, blank: blank) } ?? "empty")")
    }
}

// MARK: - Wording

enum IslandBarWording {
    /// What a filled blank reads as: the option label, "€320", "Yes".
    static func display(_ value: BarValue, blank: BarBlankInfo?) -> String {
        switch value {
        case .bool(let b): return b ? "Yes" : "No"
        case .number:
            if let unit = blank?.unit, !unit.isEmpty {
                switch unit.uppercased() {
                case "EUR": return "\u{20AC}\(value.display)"
                case "USD": return "$\(value.display)"
                case "GBP": return "\u{00A3}\(value.display)"
                default: return "\(value.display) \(unit)"
                }
            }
            return value.display
        case .text(let s):
            return blank?.options.first { $0.value == s }?.label ?? s
        }
    }

    /// "Noah gets a push. It expires in 24 hours." / "You approve."
    static func approverLine(_ review: BarReviewInfo) -> String? {
        if review.approverIsYou { return "You approve." }
        guard let name = review.approverName, !name.isEmpty else { return nil }
        return "\(name) gets a push. It expires in 24 hours."
    }
}

// MARK: - Status icon (same symbols and tints as TaskLiveActivity)

struct IslandStatusIcon: View {
    let state: String
    let size: CGFloat

    var body: some View {
        let color = Self.color(for: state)
        Image(systemName: Self.symbol(for: state))
            .font(.system(size: size * 0.55, weight: .semibold))
            .foregroundColor(color)
            .frame(width: size, height: size)
            .background(Circle().fill(color.opacity(0.15)))
            .accessibilityHidden(true)
    }

    static func symbol(for state: String) -> String {
        switch state {
        case "waiting_approval": return "hand.raised.fill"
        case "input_required": return "questionmark"
        case "completed": return "checkmark.circle.fill"
        case "failed": return "xmark"
        case "cancelled": return "stop.fill"
        default: return "cpu"
        }
    }

    static func color(for state: String) -> Color {
        switch state {
        case "waiting_approval", "input_required": return Color(red: 1, green: 0.624, blue: 0.039)
        case "completed": return Color(red: 0.188, green: 0.82, blue: 0.345)
        case "failed": return Color(red: 1, green: 0.271, blue: 0.227)
        case "cancelled": return Color(red: 0.6, green: 0.6, blue: 0.6)
        default: return Color(red: 0.039, green: 0.518, blue: 1)
        }
    }
}
