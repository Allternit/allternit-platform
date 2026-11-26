import SwiftUI
import UIKit
import ImageIO

/// Plays a GIF (image/gif) in chat and Outputs. SwiftUI's `Image` draws only the
/// first frame, so an animated picture goes through a `UIImageView`.
struct AnimatedImageView: UIViewRepresentable {
    let image: UIImage
    var fill = false

    func makeUIView(context: Context) -> UIImageView {
        let view = UIImageView()
        view.clipsToBounds = true
        view.setContentHuggingPriority(.defaultLow, for: .horizontal)
        view.setContentHuggingPriority(.defaultLow, for: .vertical)
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.setContentCompressionResistancePriority(.defaultLow, for: .vertical)
        view.isAccessibilityElement = true
        view.accessibilityLabel = "Animated picture"
        return view
    }

    func updateUIView(_ view: UIImageView, context: Context) {
        view.contentMode = fill ? .scaleAspectFill : .scaleAspectFit
        if view.image !== image { view.image = image }
        if !view.isAnimating { view.startAnimating() }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: UIImageView, context: Context) -> CGSize? {
        let size = image.size
        guard size.width > 0, size.height > 0 else { return nil }
        let width = proposal.width ?? size.width
        return CGSize(width: width, height: width * size.height / size.width)
    }

    /// An image from `data`: a GIF becomes an animated image with its per-frame
    /// delays (still, first frame, under Reduce Motion); anything else is a plain UIImage.
    static func decode(_ data: Data) -> UIImage? {
        guard data.starts(with: Array("GIF8".utf8)),
              let source = CGImageSourceCreateWithData(data as CFData, nil) else { return UIImage(data: data) }
        let count = CGImageSourceGetCount(source)
        guard count > 1, !UIAccessibility.isReduceMotionEnabled else { return UIImage(data: data) }
        var frames: [UIImage] = []
        var delays: [Double] = []
        for i in 0..<min(count, 240) {
            guard let cg = CGImageSourceCreateImageAtIndex(source, i, nil) else { continue }
            let props = CGImageSourceCopyPropertiesAtIndex(source, i, nil) as? [CFString: Any]
            let gif = props?[kCGImagePropertyGIFDictionary] as? [CFString: Any]
            let delay = (gif?[kCGImagePropertyGIFUnclampedDelayTime] as? Double) ?? (gif?[kCGImagePropertyGIFDelayTime] as? Double) ?? 0.1
            frames.append(UIImage(cgImage: cg))
            delays.append(delay < 0.02 ? 0.1 : delay)
        }
        guard frames.count > 1 else { return UIImage(data: data) }
        // Repeat each frame in proportion to its delay (10 ms steps, capped) so uneven delays hold.
        let step = max(0.02, delays.min() ?? 0.1)
        var timed: [UIImage] = []
        for (frame, delay) in zip(frames, delays) {
            timed.append(contentsOf: Array(repeating: frame, count: min(20, max(1, Int((delay / step).rounded())))))
        }
        return UIImage.animatedImage(with: timed, duration: step * Double(timed.count))
    }
}

extension UIImage {
    /// True for a decoded animated GIF (`AnimatedImageView.decode`).
    var isAnimatedPicture: Bool { (images?.count ?? 0) > 1 }
}
