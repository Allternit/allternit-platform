import SwiftUI
import UIKit

/// Passes byte reports on about 8 a second; the last one always goes through.
final class ProgressThrottle: @unchecked Sendable {
    private let send: @Sendable (ByteProgress) -> Void
    private let minGap: TimeInterval
    private let lock = NSLock()
    private var lastAt: TimeInterval = -.infinity

    init(minGap: TimeInterval = 0.12, _ send: @escaping @Sendable (ByteProgress) -> Void) {
        self.minGap = minGap
        self.send = send
    }

    /// Whether a report at `now` should go out. A complete one always does.
    static func shouldSend(now: TimeInterval, last: TimeInterval, minGap: TimeInterval, complete: Bool) -> Bool {
        complete || now - last >= minGap
    }

    func send(_ progress: ByteProgress) {
        let now = ProcessInfo.processInfo.systemUptime
        let complete = progress.totalBytes.map { progress.bytes >= $0 } ?? false
        lock.lock()
        let go = Self.shouldSend(now: now, last: lastAt, minGap: minGap, complete: complete)
        if go { lastAt = now }
        lock.unlock()
        if go { send(progress) }
    }
}

enum ByteLoading {
    /// The poster a finished `media_generate` carried: `meta.media.poster`,
    /// only as an inline image or an https link (same rule as the web card).
    static func poster(meta: [String: JSONValue]?) -> String? {
        guard case .object(let media)? = meta?["media"], case .string(let poster)? = media["poster"] else { return nil }
        let lowered = poster.lowercased()
        return lowered.hasPrefix("data:image/") || lowered.hasPrefix("https:") ? poster : nil
    }
}

/// Download "dots resolve as it loads": the poster is halftone dots that get
/// finer by the real fraction of bytes in (a band of dots until the poster
/// arrives); an unknown size holds the coarse dots and shows a small spinner. The status line gives bytes, percent and
/// ETA ("— ETA" when there is none). Reduce Motion: a fixed medium
/// halftone, and nothing animates or spins.
struct ByteLoadingView: View {
    var posterURL: String?
    var progress: ByteProgress?
    var verb = "Loading"
    /// The finished picture, once it is in: the halftone crossfades into it (0.8 s).
    var finished: UIImage?

    @ReducedMotion private var reduceMotion
    @State private var poster: UIImage?

    var body: some View {
        let line = ByteProgressText.status(verb: verb, progress ?? ByteProgress(bytes: 0))
        ZStack(alignment: .bottom) {
            Color("FillNeutral")
            if let finished {
                DotFieldView(mode: .reveal(finished, .crossfade, duration: 0.8))
            } else if let poster {
                // Halftone poster, finer by the real byte fraction; unknown size holds coarse.
                DotFieldView(mode: .halftone(poster, fraction: progress?.fraction))
            } else {
                DotFieldView(mode: .field(.band, amplitude: 0.45))
            }
            if finished == nil {
            HStack(spacing: 8) {
                if progress?.fraction == nil && !reduceMotion { ProgressView().controlSize(.mini) }
                Text(line)
                    .font(.system(size: 12.5, weight: .medium).monospacedDigit())
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(Capsule().fill(Color("BgPanel")))
            .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
            .padding(.bottom, 14)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(line)
        .task(id: posterURL) {
            guard let posterURL, poster == nil else { return }
            if let data = try? await ArtifactMediaLoader.data(for: posterURL) { poster = UIImage(data: data) }
        }
    }
}
