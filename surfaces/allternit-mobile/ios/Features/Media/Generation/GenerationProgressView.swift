import SwiftUI
import UIKit

/// How much of the generation a card shows.
enum GenerationProgressMode {
    /// The frame itself: the newest turn, and the sheet a tap opens.
    case full
    /// The compact card an older turn keeps.
    case history
}

/// Decoded previews, so a card does not decode a 320 px JPEG on every redraw.
@MainActor
enum GenerationPreviewCache {
    private static let cache = NSCache<NSString, UIImage>()

    /// Previews arrive as `data:image/...` URLs; an API path or http URL is not fetched here.
    static func image(_ preview: String) -> UIImage? {
        guard preview.hasPrefix("data:image/") else { return nil }
        let key = "\(preview.count)-\(preview.hashValue)" as NSString
        if let hit = cache.object(forKey: key) { return hit }
        guard let image = NativePaneRenderer.image(fromDataURL: preview) else { return nil }
        cache.setObject(image, forKey: key)
        return image
    }
}

/// What a picture or a motion clip is doing while it is made, one view for
/// both lanes (the iOS side of the web `GenerationProgress`, ai #1224).
///
/// It opens on the block pulse at the final aspect, with the stage and the
/// elapsed seconds along the foot, and changes only when the app reports
/// something real: a picture's preview turns to halftone dots that get finer, then
/// "Saving"; a motion clip's pulse gives way to the latest frame, "Frame N of
/// M", a bar from the fraction and the stage chips Render, Encode, Save.
/// Reduced motion: still dots and a fixed medium halftone.
struct GenerationProgressView: View {
    /// The `media_generate` tool call id (the store's key).
    let callId: String
    let lane: GenerationLane
    var mode: GenerationProgressMode = .full
    /// Width over height of the final picture; the box is reserved at this ratio.
    var aspect: CGFloat? = nil

    @ObservedObject private var store = GenerationProgressStore.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var entry: GenerationProgressStore.Entry? { store.entry(callId) }
    private var detail: GenerationProgressDetail? { entry?.detail }
    private var ratio: CGFloat { aspect ?? (lane == .picture ? 1 : 16.0 / 9.0) }

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { timeline in
            let elapsed = entry.map { max(0, Int(timeline.date.timeIntervalSince($0.startedAt))) } ?? 0
            Group {
                switch mode {
                case .full: full(elapsed: elapsed)
                case .history: lane == .picture ? AnyView(historyPicture) : AnyView(historyMotion)
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(lane == .picture ? "Making a picture" : "Making a clip")
        .accessibilityValue(accessibilityValue)
    }

    private var accessibilityValue: String {
        let line = GenerationProgressDetail.mainLine(detail, lane: lane)
        if let f = GenerationProgressDetail.barFraction(detail) { return "\(line), \(Int((f * 100).rounded())) percent" }
        return line
    }

    // MARK: - Full frame

    private func full(elapsed: Int) -> some View {
        let main = GenerationProgressDetail.mainLine(detail, lane: lane)
        return Color("FillNeutral")
            .aspectRatio(ratio, contentMode: .fit)
            .frame(maxWidth: .infinity)
            .overlay { art(detail: detail, thumb: nil) }
            .overlay(alignment: .topLeading) {
                if lane == .motion { StageChips(stage: detail?.stage).padding(10) }
            }
            .overlay(alignment: .bottom) {
                VStack(spacing: 0) {
                    HStack(spacing: 8) {
                        Text(main)
                            .font(.system(size: 12.5))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        Spacer(minLength: 4)
                        Text("\(elapsed)s")
                            .font(.system(size: 12.5).monospacedDigit())
                            .foregroundColor(Color("TextTertiary"))
                    }
                    .padding(.horizontal, 14)
                    .padding(.top, 22)
                    .padding(.bottom, 10)
                    .background(LinearGradient(stops: [.init(color: Color("BgPrimary"), location: 0.45), .init(color: Color("BgPrimary").opacity(0), location: 1)], startPoint: .bottom, endPoint: .top))
                    ProgressBar(fraction: GenerationProgressDetail.barFraction(detail), reduceMotion: reduceMotion)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    /// The picture of the generation: the pulse until something real arrives.
    @ViewBuilder
    private func art(detail: GenerationProgressDetail?, thumb: String?) -> some View {
        let source = thumb ?? ((detail?.hasPreview ?? false) ? detail?.preview : nil)
        if let source, let image = GenerationPreviewCache.image(source) {
            if lane == .picture && thumb == nil {
                // Option B: the preview as halftone dots, finer by the real fraction.
                DotFieldView(mode: .halftone(image, fraction: Self.halftoneFraction(detail)))
            } else {
                GeometryReader { proxy in
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFill()
                        .frame(width: proxy.size.width, height: proxy.size.height)
                        .clipped()
                }
            }
        } else {
            // Option C: a light band sweeps the dots; half amplitude while queued.
            let queued = detail == nil || detail?.stage == .queued
            DotFieldView(mode: .field(.band, amplitude: queued ? 0.45 : 1))
        }
    }

    /// Real fraction of a picture preview: solid once saving, held coarse when unknown.
    static func halftoneFraction(_ detail: GenerationProgressDetail?) -> Double? {
        if detail?.stage == .saving || detail?.stage == .downloading { return 1 }
        return detail?.fraction
    }

    // MARK: - History cards

    private var historyPicture: some View {
        let at = GenerationProgressDetail.pictureStepIndex(detail?.stage)
        let rows = ["Queued", "Drawing", "Refining", "Saving"]
        let label = GenerationProgressDetail.stageLabel(detail, lane: .picture)
        return HStack(spacing: 12) {
            Color("FillNeutral")
                .frame(width: 84, height: 84)
                .overlay { art(detail: detail, thumb: nil) }
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            VStack(alignment: .leading, spacing: 7) {
                ForEach(Array(rows.enumerated()), id: \.offset) { k, name in
                    StepRow(state: k < at ? .done : (k == at ? .on : .todo),
                            text: (k == at && k >= 2 && !(detail?.label.trimmingCharacters(in: .whitespaces).isEmpty ?? true)) ? label : name,
                            reduceMotion: reduceMotion)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(Color("BgPrimary"))
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private var historyMotion: some View {
        let label = GenerationProgressDetail.stageLabel(detail, lane: .motion)
        let main = GenerationProgressDetail.mainLine(detail, lane: .motion)
        let counting = main != label
        var shots: [String] = entry?.thumbs ?? []
        if shots.isEmpty, let preview = detail?.preview { shots = [preview] }
        let thumbs = Array(shots.suffix(GenerationProgressStore.maxThumbs))
        var bare = detail
        bare?.label = ""
        let headline = counting ? GenerationProgressDetail.stageLabel(bare, lane: .motion) : label
        let rendering = GenerationProgressDetail.motionStageIndex(detail?.stage) == 0
        return VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text(headline)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Spacer(minLength: 4)
                if counting {
                    Text(main.lowercased())
                        .font(.system(size: 13).monospacedDigit())
                        .foregroundColor(Color("TextTertiary"))
                }
            }
            if thumbs.isEmpty {
                Color("FillNeutral")
                    .aspectRatio(16.0 / 9.0, contentMode: .fit)
                    .overlay { DotFieldView(mode: .field(.band, amplitude: 1)) }
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            } else {
                HStack(spacing: 6) {
                    ForEach(Array(thumbs.enumerated()), id: \.offset) { k, src in
                        Color("FillNeutral")
                            .aspectRatio(16.0 / 9.0, contentMode: .fit)
                            .overlay { art(detail: nil, thumb: src) }
                            .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                                .stroke(k == thumbs.count - 1 && rendering ? Color("TextPrimary") : Color("BorderSubtle"),
                                        lineWidth: k == thumbs.count - 1 && rendering ? 2 : 1))
                    }
                }
            }
            ProgressBar(fraction: GenerationProgressDetail.barFraction(detail), reduceMotion: reduceMotion, rounded: true)
        }
        .padding(12)
        .background(Color("BgPrimary"))
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}

// MARK: - Pieces

/// The thin bar along the foot; nothing when no fraction is known.
private struct ProgressBar: View {
    let fraction: Double?
    let reduceMotion: Bool
    var rounded = false

    var body: some View {
        if let fraction {
            GeometryReader { proxy in
                ZStack(alignment: .leading) {
                    Rectangle().fill(Color("FillNeutral"))
                    Rectangle().fill(Color("TextPrimary"))
                        .frame(width: proxy.size.width * fraction)
                        .animation(reduceMotion ? nil : .easeOut(duration: 0.3), value: fraction)
                }
            }
            .frame(height: 3)
            .clipShape(RoundedRectangle(cornerRadius: rounded ? 2 : 0))
            .accessibilityHidden(true)
        }
    }
}

/// Motion stage chips, in order: Render, Encode, Save.
private struct StageChips: View {
    let stage: GenerationStage?
    private static let names = ["Render", "Encode", "Save"]

    var body: some View {
        let at = GenerationProgressDetail.motionStageIndex(stage)
        HStack(spacing: 6) {
            ForEach(Array(Self.names.enumerated()), id: \.offset) { k, name in
                Text((k < at ? "✓ " : "") + name)
                    .font(.system(size: 11.5))
                    .foregroundColor(k == at ? Color("TextPrimary") : (k < at ? Color("TextSecondary") : Color("TextTertiary")))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(Capsule().fill(k == at ? Color("FillNeutral") : Color("BgPrimary")))
                    .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
            }
        }
    }
}

private struct StepRow: View {
    enum Phase { case done, on, todo }
    let state: Phase
    let text: String
    let reduceMotion: Bool

    var body: some View {
        HStack(spacing: 8) {
            Group {
                switch state {
                case .done: Image(systemName: "checkmark").font(.system(size: 10, weight: .semibold))
                case .on:
                    if reduceMotion { Image(systemName: "circle.dotted").font(.system(size: 11)) }
                    else { ProgressView().controlSize(.mini) }
                case .todo: Circle().fill(Color("FillNeutral")).frame(width: 6, height: 6)
                }
            }
            .frame(width: 14, height: 14)
            Text(text)
                .font(.system(size: 13))
                .lineLimit(1)
        }
        .foregroundColor(state == .on ? Color("TextPrimary") : Color("TextTertiary"))
    }
}
