import SwiftUI
import UIKit

/// Review & sign: the signature lifts out of the pad, shrinks and flies to its
/// slot, then a gold shimmer sweeps across the ink and a success haptic fires
/// (web `after.tsx` flight, `sign-clip-beats.md` beats 6-7).
///
/// Both rects are in **global** (window) coordinates: `from` is the ink bounds
/// reported by `SignSheetView` (`SignSheetResult.sourceRect`), `to` is the slot
/// on the page. Overlay it on a full-screen container after the sheet closes:
///
///     .overlay { if let r = result { SignFlight(image: r.image, from: r.sourceRect, to: slotRect) { placed = true } } }
///
/// `onFinished` runs once, after the shimmer; swap in the placed signature
/// there. Reduce Motion jumps to the end (haptic and `onFinished` immediately).
struct SignFlight: View {
    let image: UIImage
    let from: CGRect
    let to: CGRect
    var onFinished: () -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var landed = false
    @State private var sweep: CGFloat = -0.4
    @State private var started = false

    static let flightSeconds = 0.7
    static let shimmerSeconds = 1.1

    var body: some View {
        GeometryReader { proxy in
            let origin = proxy.frame(in: .global).origin
            let target = Self.fit(image.size, in: to)
            let rect = (landed ? target : Self.fit(image.size, in: from)).offsetBy(dx: -origin.x, dy: -origin.y)
            ZStack {
                Image(uiImage: image)
                    .resizable()
                    .renderingMode(.original)
                Image(uiImage: image)
                    .resizable()
                    .renderingMode(.template)
                    .foregroundStyle(Color(hex: "#C9A45C"))
                    .mask(alignment: .leading) {
                        GeometryReader { g in
                            LinearGradient(colors: [.clear, .white, .clear], startPoint: .leading, endPoint: .trailing)
                                .frame(width: g.size.width * 0.3)
                                .offset(x: g.size.width * sweep)
                        }
                    }
                    .opacity(sweep > -0.4 ? 1 : 0)
            }
            .frame(width: rect.width, height: rect.height)
            .position(x: rect.midX, y: rect.midY)
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
        .task { await run() }
    }

    @MainActor private func run() async {
        guard !started else { return }
        started = true
        if reduceMotion {
            landed = true
            Haptics.success()
            onFinished()
            return
        }
        withAnimation(.timingCurve(0.32, 0.72, 0, 1, duration: Self.flightSeconds)) { landed = true }
        try? await Task.sleep(nanoseconds: UInt64(Self.flightSeconds * 1_000_000_000))
        Haptics.success()
        withAnimation(.easeInOut(duration: Self.shimmerSeconds)) { sweep = 1.1 }
        try? await Task.sleep(nanoseconds: UInt64((Self.shimmerSeconds + 0.15) * 1_000_000_000))
        onFinished()
    }

    /// Aspect-fits `size` inside `rect`, centred.
    static func fit(_ size: CGSize, in rect: CGRect) -> CGRect {
        guard size.width > 0, size.height > 0, rect.width > 0, rect.height > 0 else { return rect }
        let k = min(rect.width / size.width, rect.height / size.height)
        let w = size.width * k, h = size.height * k
        return CGRect(x: rect.midX - w / 2, y: rect.midY - h / 2, width: w, height: h)
    }
}
