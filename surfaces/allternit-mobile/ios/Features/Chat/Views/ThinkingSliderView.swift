import SwiftUI

/// The model's own thinking levels on Allternit's block grid: vendor tint,
/// coral head block, the vendor's word and level names (model-thinking.json,
/// served on each catalog row). Same design as the web ThinkingSlider.
struct ThinkingSliderView: View {
    let thinking: ModelThinking
    let value: String
    let onChange: (String) -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

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
    private var mono: Bool { thinking.vendor == "openai" }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                RoundedRectangle(cornerRadius: 2).fill(Self.tint(thinking.vendor)).frame(width: 7, height: 7)
                Text(thinking.term).font(.system(size: 12.5, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                Spacer()
                Text("\(thinking.vendorName ?? "") · default \(thinking.label(thinking.default))")
                    .font(.system(size: 11)).foregroundColor(Color("TextTertiary")).lineLimit(1)
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
                            UISelectionFeedbackGenerator().selectionChanged()
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
            if let note = thinking.note, !note.isEmpty {
                Text(note).font(.system(size: 10.5)).foregroundColor(Color("TextTertiary"))
            }
        }
        .padding(10)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
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
        ratio >= 0.95 ? Theme.statusError : ratio >= 0.8 ? Theme.statusWarning : Color.green
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
