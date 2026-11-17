#if DEBUG
import SwiftUI

/// DEBUG gallery for the calm foundations. Launch with `-calm-gallery`.
enum CalmGallery {
    static var isEnabled: Bool { launchArgumentEnabled(LaunchArgument.calmGallery) }
}

struct CalmGalleryView: View {
    @ReducedMotion private var reduced
    @AppStorage(Haptics.enabledKey) private var haptics = true
    @State private var on = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                Text("Glass").font(.headline)
                GlassContainer {
                    HStack(spacing: 12) {
                        Text("Pill").padding(.horizontal, 18).padding(.vertical, 10).glassSurface(cornerRadius: 22)
                        Text("Card").padding(18).glassSurface(cornerRadius: 16)
                    }
                }
                .padding().background(LinearGradient(colors: [.orange, .pink, .purple], startPoint: .topLeading, endPoint: .bottomTrailing))

                Text("Haptics").font(.headline)
                Toggle("Haptic feedback", isOn: $haptics)
                HStack {
                    Button("light") { Haptics.light() }
                    Button("medium") { Haptics.medium() }
                    Button("soft") { Haptics.soft() }
                    Button("select") { Haptics.selection() }
                    Button("ok") { Haptics.success() }
                    Button("warn") { Haptics.warning() }
                }.buttonStyle(.bordered).font(.footnote)

                Text("Motion (reduced: \(reduced ? "yes" : "no"))").font(.headline)
                ForEach(Array(presets.enumerated()), id: \.offset) { _, p in
                    HStack {
                        Text(p.0).frame(width: 90, alignment: .leading)
                        Circle().fill(.tint).frame(width: 22, height: 22)
                            .offset(x: on ? 180 : 0)
                            .animation(Motion.pick(p.1, reduced: reduced), value: on)
                    }
                }
                Button("Play") { on.toggle() }.buttonStyle(.borderedProminent)
                Text("fanStagger = \(Motion.fanStagger)s").font(.footnote).foregroundStyle(.secondary)
            }
            .padding()
        }
    }

    private var presets: [(String, Animation)] {
        [("drawer", Motion.drawer), ("fan", Motion.fan), ("deck", Motion.deck), ("fade", Motion.fade)]
    }
}
#endif
