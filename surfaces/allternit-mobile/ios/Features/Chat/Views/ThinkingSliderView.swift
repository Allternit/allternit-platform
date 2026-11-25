import SwiftUI

/// The model's own thinking levels on Allternit's block grid: vendor tint,
/// coral head block, the vendor's word and level names (model-thinking.json,
/// served on each catalog row). Same design as the web ThinkingSlider.
struct ThinkingSliderView: View {
    let thinking: ModelThinking
    let value: String
    let onChange: (String) -> Void
    /// A cost line under the slider. Shown only when the catalog knows the
    /// cost; nil (the default) shows nothing.
    var costLine: String? = nil
    @ReducedMotion private var reduceMotion

    private static let cols = 30
    private static let rows = 5
    private static let core = Color(red: 0.851, green: 0.467, blue: 0.341)

    static func tint(_ vendor: String) -> Color {
        switch vendor {
        case "claude": return Color(red: 0.788, green: 0.392, blue: 0.259)
        case "google": return Color(red: 0.435, green: 0.498, blue: 0.941)
        case "deepseek": return Color(red: 0.302, green: 0.420, blue: 0.996)
        case "zai": return Color(red: 0.220, green: 0.349, blue: 1.0)
        default: return Color("TextPrimary")
        }
    }

    private var index: Int { max(0, thinking.levels.firstIndex { $0.id == value } ?? 0) }
    private var fill: Double {
        let n = thinking.levels.count
        return n <= 1 ? 1 : max(0.06, Double(index) / Double(n - 1))
    }
    /// The top level of a real scale (3+ levels) gets the art state.
    private var isTopLevel: Bool { thinking.levels.count >= 3 && index == thinking.levels.count - 1 }
    private var mono: Bool { thinking.vendor == "openai" }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                RoundedRectangle(cornerRadius: 2).fill(Self.tint(thinking.vendor)).frame(width: 7, height: 7)
                Text(thinking.term).font(.system(size: 12.5, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                Spacer()
                if value == thinking.default {
                    // The Default mark: the level the vendor picks for you.
                    Text("Default")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 6).padding(.vertical, 1)
                        .background(Color("BgPanel"), in: Capsule())
                    Text(thinking.vendorName ?? "")
                        .font(.system(size: 11)).foregroundColor(Color("TextTertiary")).lineLimit(1)
                } else {
                    Text("\(thinking.vendorName ?? "") · default \(thinking.label(thinking.default))")
                        .font(.system(size: 11)).foregroundColor(Color("TextTertiary")).lineLimit(1)
                }
            }
            GeometryReader { proxy in
                grid
                    .contentShape(Rectangle())
                    .gesture(DragGesture(minimumDistance: 0).onChanged { g in
                        let n = thinking.levels.count
                        guard n > 0 else { return }
                        let i = Int((g.location.x / max(1, proxy.size.width) * Double(n - 1)).rounded())
                        let level = thinking.levels[min(n - 1, max(0, i))]
                        if level.id != value {
                            Haptics.selection()
                            onChange(level.id)
                        }
                    })
            }
            .frame(height: 30)
            .padding(6)
            .background(Color("BgPanel"), in: RoundedRectangle(cornerRadius: 12))
            HStack(spacing: 0) {
                ForEach(Array(thinking.levels.enumerated()), id: \.element.id) { i, level in
                    Button(action: { onChange(level.id) }) {
                        Text(mono ? level.id : level.label)
                            .font(.system(size: thinking.levels.count > 5 ? 10.5 : 11.5, weight: i == index ? .semibold : .regular, design: mono ? .monospaced : .default))
                            .foregroundColor(i == index ? Self.tint(thinking.vendor) : Color("TextTertiary"))
                            .lineLimit(1)
                            .minimumScaleFactor(0.8)
                    }
                    .buttonStyle(.plain)
                    if i < thinking.levels.count - 1 { Spacer(minLength: 2) }
                }
            }
            if isTopLevel {
                ThinkingArtRow(tint: Self.tint(thinking.vendor), core: Self.core)
                    .transition(.opacity)
            }
            if let note = thinking.note, !note.isEmpty {
                Text(note).font(.system(size: 10.5)).foregroundColor(Color("TextTertiary"))
            }
            if let costLine, !costLine.isEmpty {
                Text(costLine).font(.system(size: 10.5)).foregroundColor(Color("TextTertiary"))
            }
        }
        .padding(10)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
        .animation(reduceMotion ? nil : .easeOut(duration: 0.2), value: isTopLevel)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(thinking.term)
        .accessibilityValue(thinking.label(value))
        .accessibilityAdjustableAction { direction in
            let n = thinking.levels.count
            let next = direction == .increment ? min(n - 1, index + 1) : max(0, index - 1)
            onChange(thinking.levels[next].id)
        }
    }

    private var grid: some View {
        let tint = Self.tint(thinking.vendor)
        return VStack(spacing: 2) {
            ForEach(0..<Self.rows, id: \.self) { _ in
                HStack(spacing: 2) {
                    ForEach(0..<Self.cols, id: \.self) { c in
                        let x = Double(c) / Double(Self.cols - 1)
                        let on = x <= fill + 0.001
                        let head = on && fill - x < 1.5 / Double(Self.cols)
                        RoundedRectangle(cornerRadius: 1)
                            .fill(head ? Self.core : tint)
                            .opacity(head ? 1 : on ? 0.28 + 0.72 * (x / max(fill, 0.01)) : 0)
                    }
                }
            }
        }
        .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: fill)
    }
}

/// The art state for the top effort level: a strip of our blocks. A
/// dissolve band sweeps left to right over the right 40% in 0.25 s steps
/// (about a 3 s loop) and a few coral blocks spark. Still under reduced motion.
private struct ThinkingArtRow: View {
    let tint: Color
    let core: Color
    @ReducedMotion private var reduceMotion

    private static let cols = 30
    private static let rows = 3

    var body: some View {
        Group {
            if reduceMotion {
                strip(step: 0)
            } else {
                TimelineView(.periodic(from: .now, by: 0.25)) { ctx in
                    strip(step: Int(ctx.date.timeIntervalSinceReferenceDate / 0.25))
                }
            }
        }
        .frame(height: 22)
        .accessibilityHidden(true)
    }

    private func strip(step: Int) -> some View {
        let cols = Self.cols
        let sweepStart = Int(Double(cols) * 0.6)
        let sweepLen = cols - sweepStart
        let phase = step % 12                       // 12 steps of 0.25 s = 3 s
        let sweep = sweepStart + Int(Double(phase) / 11.0 * Double(sweepLen))
        return VStack(spacing: 2) {
            ForEach(0..<Self.rows, id: \.self) { r in
                HStack(spacing: 2) {
                    ForEach(0..<cols, id: \.self) { c in
                        let noise = (c * 7 + r * 13 + step * 3) % 5
                        let dissolved = c >= sweepStart && c < sweep && noise < 3
                        let spark = (c * 11 + r * 5 + step * 7) % 23 == 0
                        RoundedRectangle(cornerRadius: 1)
                            .fill(spark ? core : tint)
                            .opacity(dissolved ? 0.08 : spark ? 1 : 0.35 + 0.65 * Double(c) / Double(cols - 1))
                    }
                }
            }
        }
    }
}

/// "Effort · 5" chip for a model row.
struct ThinkingChip: View {
    let thinking: ModelThinking
    var body: some View {
        Text("\(thinking.term) · \(thinking.levels.count)")
            .font(.system(size: 10))
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .overlay(RoundedRectangle(cornerRadius: 7).stroke(ThinkingSliderView.tint(thinking.vendor).opacity(0.5), lineWidth: 1))
    }
}

/// A provider mark inside its usage ring: green, amber at 80%, red at 95%.
struct LimitRingView<Content: View>: View {
    let window: ProviderLimitWindow?
    var inner: ProviderLimitWindow? = nil
    let size: CGFloat
    @ViewBuilder var content: () -> Content

    static func color(_ ratio: Double) -> Color {
        ratio >= UsageThresholds.landAt ? Theme.statusError : ratio >= UsageThresholds.warnAt ? Theme.statusWarning : Color.green
    }

    var body: some View {
        let width: CGFloat = size >= 48 ? 4 : 2.5
        ZStack {
            Circle().stroke(Color("BorderSubtle"), lineWidth: width)
            if let window {
                Circle()
                    .trim(from: 0, to: window.usedRatio)
                    .stroke(Self.color(window.usedRatio), style: StrokeStyle(lineWidth: width, lineCap: .round))
                    .rotationEffect(.degrees(-90))
            }
            if let inner {
                Circle()
                    .trim(from: 0, to: inner.usedRatio)
                    .stroke(Self.color(inner.usedRatio), style: StrokeStyle(lineWidth: width * 0.75, lineCap: .round))
                    .rotationEffect(.degrees(-90))
                    .padding(width * 1.8)
            }
            content()
        }
        .frame(width: size, height: size)
    }
}
