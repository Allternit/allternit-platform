import SwiftUI

/// The listening panel under dictation: a level waveform that follows the
/// mic, and Cancel / Stop / Keep mine. No rewrite step, the words in the
/// field are the user's own. White + neutral fill, graphite in dark.
struct DictationPanel: View {
    let onCancel: () -> Void
    let onStop: () -> Void
    let onKeep: () -> Void

    @State private var levels: [Double] = Array(repeating: 0, count: DictationPanel.barCount)
    @State private var heard: Double = 0
    @ReducedMotion private var reduceMotion

    private static let barCount = 36

    var body: some View {
        VStack(spacing: 10) {
            HStack(spacing: 6) {
                Circle().fill(Theme.statusError).frame(width: 6, height: 6)
                Text("Listening")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
            }
            waveform
                .frame(height: 28)
                .accessibilityHidden(true)
            HStack(spacing: 10) {
                Button("Cancel", action: onCancel)
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(minHeight: 44)
                    .buttonStyle(.plain)
                Spacer()
                Button(action: onStop) {
                    Image(systemName: "stop.fill")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(.white)
                        .frame(width: 44, height: 44)
                        .background(Theme.statusError, in: Circle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Stop dictation")
                Spacer()
                Button("Keep mine", action: onKeep)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 14)
                    .frame(minHeight: 44)
                    .background(Color("FillNeutral"), in: Capsule())
                    .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 12)
        .padding(.bottom, 4)
        .background(Color("BgPanel"), in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        .overlay(
            // The border warms toward the accent while speech is heard.
            RoundedRectangle(cornerRadius: 22, style: .continuous)
                .stroke(Color("AccentPrimary").opacity(0.15 + 0.6 * heard), lineWidth: 1)
        )
        .padding(.horizontal, 16)
        .padding(.bottom, 8)
        .accessibilityElement(children: .contain)
        .task {
            // Sample the mic level ~20x a second into a short history.
            while !Task.isCancelled {
                let level = VoiceLevels.shared.micLevel()
                var next = levels
                next.removeFirst()
                next.append(level)
                if reduceMotion {
                    levels = next
                    heard = level
                } else {
                    withAnimation(.linear(duration: 0.05)) {
                        levels = next
                        heard = level
                    }
                }
                try? await Task.sleep(for: .milliseconds(reduceMotion ? 100 : 50))
            }
        }
    }

    private var waveform: some View {
        GeometryReader { proxy in
            let gap: CGFloat = 3
            let w = max(2, (proxy.size.width - gap * CGFloat(Self.barCount - 1)) / CGFloat(Self.barCount))
            HStack(alignment: .center, spacing: gap) {
                ForEach(0..<Self.barCount, id: \.self) { i in
                    Capsule()
                        .fill(Color("AccentPrimary").opacity(0.35 + 0.65 * levels[i]))
                        .frame(width: w, height: max(3, proxy.size.height * (0.1 + 0.9 * levels[i])))
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

/// "Undo dictation": a pill that appears after dictation stops and puts the
/// field back to what it held before. Goes away after a few seconds.
struct UndoDictationPill: View {
    let onUndo: () -> Void
    let onExpire: () -> Void

    var body: some View {
        HStack {
            Button(action: onUndo) {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.uturn.backward").font(.system(size: 12, weight: .semibold))
                    Text("Undo dictation").font(.system(size: 13, weight: .medium))
                }
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 14)
                .frame(minHeight: 44)
                .background(Color("FillNeutral"), in: Capsule())
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
            }
            .buttonStyle(.plain)
            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 6)
        .task {
            try? await Task.sleep(for: .seconds(8))
            if !Task.isCancelled { onExpire() }
        }
    }
}
