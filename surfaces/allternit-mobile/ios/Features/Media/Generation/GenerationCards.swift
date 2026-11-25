import SwiftUI

/// Press-and-hold peek, tap to open: the touch version of the web card's
/// hover peek. Holding the card for 350 ms shows the full frame over it;
/// releasing closes it; a tap opens the full view in a sheet.
struct GenerationPeekModifier: ViewModifier {
    let callId: String
    let lane: GenerationLane
    var aspect: CGFloat? = nil

    /// How long to hold before the peek opens (the web hover waits 200 ms; touch holds 350 ms).
    static let holdSeconds = 0.35

    @State private var peeking = false
    @State private var opened = false

    func body(content: Content) -> some View {
        content
            .contentShape(Rectangle())
            .onTapGesture { opened = true }
            .onLongPressGesture(minimumDuration: Self.holdSeconds, maximumDistance: 40, perform: {
                peeking = true
                Haptics.light()
            }, onPressingChanged: { pressing in
                if !pressing { peeking = false }
            })
            .overlay(alignment: .top) {
                if peeking {
                    GenerationProgressView(callId: callId, lane: lane, mode: .full, aspect: aspect)
                        .frame(width: 280)
                        .shadow(color: .black.opacity(0.18), radius: 14, y: 6)
                        .transition(.opacity)
                        .allowsHitTesting(false)
                }
            }
            .zIndex(peeking ? 1 : 0)
            .sheet(isPresented: $opened) {
                GenerationSheet(callId: callId, lane: lane, aspect: aspect)
            }
            .accessibilityAddTraits(.isButton)
            .accessibilityHint("Opens the full view")
    }
}

extension View {
    /// Tap opens the full generation view; press and hold peeks at it.
    func generationPeek(callId: String, lane: GenerationLane, aspect: CGFloat? = nil) -> some View {
        modifier(GenerationPeekModifier(callId: callId, lane: lane, aspect: aspect))
    }
}

/// The full view a tap opens, in a bottom sheet on the phone. Closes itself
/// when the generation ends.
struct GenerationSheet: View {
    let callId: String
    let lane: GenerationLane
    var aspect: CGFloat? = nil

    @ObservedObject private var store = GenerationProgressStore.shared
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 14) {
            Text(lane == .picture ? "Making a picture" : "Making a clip")
                .font(.headline)
                .foregroundColor(Color("TextPrimary"))
                .padding(.top, 18)
            GenerationProgressView(callId: callId, lane: lane, mode: .full, aspect: aspect)
                .padding(.horizontal, 16)
            if let detail = store.entry(callId)?.detail, detail.stage == .downloading, let line = Self.bytesLine(detail) {
                Text(line).font(.footnote.monospacedDigit()).foregroundColor(Color("TextSecondary"))
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity)
        .background(Color("BgPrimary").ignoresSafeArea())
        .presentationDetents([.medium, .large])
        .onChange(of: store.entry(callId) == nil) { _, ended in
            if ended { dismiss() }
        }
    }

    /// "3.2 of 8.0 MB, 12s left"; the total and the ETA only when known.
    static func bytesLine(_ detail: GenerationProgressDetail) -> String? {
        guard let bytes = detail.bytes else { return nil }
        let fmt = ByteCountFormatter()
        fmt.countStyle = .file
        var line = fmt.string(fromByteCount: Int64(bytes))
        if let total = detail.totalBytes { line += " of \(fmt.string(fromByteCount: Int64(total)))" }
        line += detail.etaSeconds.map { ", \(Int($0.rounded()))s left" } ?? ""
        return line
    }
}

/// A running `media_generate` call drawn in the chat: the progress card while
/// it runs (full for the newest turn, compact for older ones), the failure
/// card in the media frame if it fails. A call that finished draws nothing
/// here; its result is the picture or clip itself.
struct MediaGenerateCard: View {
    let call: ToolCallRecord
    let isNewestTurn: Bool
    /// The reply is still streaming (a stopped or backgrounded reply shows no spinner).
    let replyIsLive: Bool
    var onChooseModel: () -> Void = {}
    var onRetry: () -> Void = {}
    /// True while the finish reveal plays; the chat holds the bubble's picture until it ends.
    var onRevealing: (Bool) -> Void = { _ in }

    @ObservedObject private var store = GenerationProgressStore.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// The preview the card last drew as halftone (picture lane). Non-nil means
    /// a halftone preview was showing, so the finish crossfades instead of blooming.
    @State private var halftoneShown: UIImage?
    @State private var reveal: Reveal?

    struct Reveal: Equatable {
        let image: UIImage
        let style: DotFieldView.RevealStyle
        static func == (a: Reveal, b: Reveal) -> Bool { a.image === b.image && a.style == b.style }
    }

    var body: some View {
        content
            .onChange(of: store.entry(call.id)?.detail.preview) { _, _ in noteHalftone() }
            .onChange(of: call.state) { old, new in
                if old == .running, new == .done { Task { await playReveal() } }
            }
            .onDisappear {
                // The card left mid-reveal: let go of the bubble now.
                if reveal != nil { reveal = nil; onRevealing(false) }
            }
    }

    @ViewBuilder
    private var content: some View {
        switch call.state {
        case .done where reveal != nil:
            if let reveal {
                Color("FillNeutral")
                    .aspectRatio(Self.aspect(of: call) ?? 1, contentMode: .fit)
                    .frame(maxWidth: .infinity)
                    .overlay { DotFieldView(mode: .reveal(reveal.image, reveal.style, duration: Self.revealSeconds)) }
                    .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                    .accessibilityHidden(true)
            }
        case .running where replyIsLive:
            let lane = Self.lane(of: call, reported: store.entry(call.id)?.detail)
            let card = GenerationProgressView(callId: call.id, lane: lane, mode: isNewestTurn ? .full : .history, aspect: Self.aspect(of: call))
            if isNewestTurn { card.generationPeekTap(callId: call.id, lane: lane, aspect: Self.aspect(of: call)) }
            else { card.generationPeek(callId: call.id, lane: lane, aspect: Self.aspect(of: call)) }
        case .failed:
            GenerationFailureView(
                info: ChatErrorInfo.make(message: call.error, lane: Self.lane(of: call, reported: nil) == .motion ? .motion : .picture),
                lane: Self.lane(of: call, reported: nil),
                aspect: Self.aspect(of: call),
                onChooseModel: onChooseModel,
                onRetry: onRetry
            )
        default:
            EmptyView()
        }
    }

    static let revealSeconds = 0.8

    /// Remember the preview while it is drawn as halftone (picture lane only).
    private func noteHalftone() {
        guard let detail = store.entry(call.id)?.detail, detail.lane == .picture, detail.hasPreview,
              let source = detail.preview, let image = GenerationPreviewCache.image(source) else { return }
        halftoneShown = image
    }

    /// The finished picture takes the card's place: dots bloom into it when no
    /// preview was shown, a crossfade out of the halftone when one was. The real
    /// picture is read from the call's result; the last preview stands in if it
    /// cannot be. Reduce Motion: no reveal, the picture just appears.
    private func playReveal() async {
        guard !reduceMotion, Self.lane(of: call, reported: nil) == .picture else { return }
        var final: UIImage?
        if let url = Self.imageURL(in: call.output), let data = try? await ArtifactMediaLoader.data(for: url) { final = UIImage(data: data) }
        let style: DotFieldView.RevealStyle = halftoneShown != nil ? .crossfade : .cells
        guard let image = final ?? halftoneShown else { return }
        reveal = Reveal(image: image, style: style)
        onRevealing(true)
        // Runs on every exit, including a cancelled sleep, so the hold always lets go.
        defer {
            reveal = nil
            halftoneShown = nil
            onRevealing(false)
        }
        try? await Task.sleep(nanoseconds: UInt64(Self.revealSeconds * 1_000_000_000))
    }

    /// The first inline picture or https picture link in a tool result.
    static func imageURL(in output: String?) -> String? {
        guard let output else { return nil }
        let pattern = #"(data:image/[a-zA-Z+.-]+;base64,[A-Za-z0-9+/=]+|https://[^\s"')\]]+\.(?:png|jpe?g|webp)(?:\?[^\s"')\]]*)?)"#
        guard let re = try? NSRegularExpression(pattern: pattern),
              let m = re.firstMatch(in: output, range: NSRange(output.startIndex..., in: output)),
              let r = Range(m.range, in: output) else { return nil }
        return String(output[r])
    }

    /// The lane: what the app reported, else the call's own `kind`.
    static func lane(of call: ToolCallRecord, reported: GenerationProgressDetail?) -> GenerationLane {
        if let reported { return reported.lane }
        return call.input["kind"]?.lowercased() == "video" ? .motion : .picture
    }

    /// The final picture's shape from the call's width and height, when it gave them.
    static func aspect(of call: ToolCallRecord) -> CGFloat? {
        guard let w = call.input["width"].flatMap(Double.init), let h = call.input["height"].flatMap(Double.init), w > 0, h > 0 else { return nil }
        return CGFloat(min(3, max(1.0 / 3.0, w / h)))
    }
}

extension View {
    /// The newest turn's frame: a tap opens the sheet (no peek, it is already full).
    func generationPeekTap(callId: String, lane: GenerationLane, aspect: CGFloat?) -> some View {
        modifier(GenerationTapModifier(callId: callId, lane: lane, aspect: aspect))
    }
}

struct GenerationTapModifier: ViewModifier {
    let callId: String
    let lane: GenerationLane
    var aspect: CGFloat?
    @State private var opened = false

    func body(content: Content) -> some View {
        content
            .contentShape(Rectangle())
            .onTapGesture { opened = true }
            .sheet(isPresented: $opened) { GenerationSheet(callId: callId, lane: lane, aspect: aspect) }
            .accessibilityAddTraits(.isButton)
            .accessibilityHint("Opens the full view")
    }
}

/// A picture or clip that did not get made (errors, design B): the reason in
/// the server's words, when a usage limit resets, and the way out, drawn in
/// the media frame at the shape the result would have had.
struct GenerationFailureView: View {
    let info: ChatErrorInfo
    let lane: GenerationLane
    var aspect: CGFloat? = nil
    var onChooseModel: () -> Void = {}
    var onRetry: () -> Void = {}

    @ObservedObject private var connections = ConnectionsStore.shared
    @State private var sheet: ConnSheet?

    private var errorLane: ChatErrorInfo.Lane { lane == .motion ? .motion : .picture }

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 20))
                .foregroundColor(Theme.statusWarning)
                .accessibilityHidden(true)
            Text(info.title(lane: errorLane))
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .multilineTextAlignment(.center)
            Text(info.reason)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            if let label = info.actionLabel {
                Button {
                    Haptics.light()
                    perform()
                } label: {
                    Text(label)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(.black)
                        .padding(.horizontal, 14)
                        .frame(height: 30)
                        .background(Capsule().fill(Color("AccentPrimary")))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, minHeight: 120)
        .background(Color("BgPrimary"))
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Theme.statusWarning.opacity(0.4), lineWidth: 1))
        .accessibilityElement(children: .contain)
        .sheet(item: $sheet) { s in ConnectSheetView(sheet: s, store: connections) }
    }

    private func perform() {
        switch info.action {
        case .connect:
            if info.isChatGPT { sheet = .chatgptDevice } else { DeepLinkRouter.shared.enqueue(.settings("subscriptions")) }
        case .pickModel: onChooseModel()
        case .retry: onRetry()
        case nil: break
        }
    }
}

/// A compact strip above the composer while generations run: one chip per
/// running picture or clip. Tap opens the full view in a sheet; press and
/// hold peeks at the frame, release closes it.
struct RunningGenerationsStrip: View {
    @ObservedObject private var store = GenerationProgressStore.shared
    @State private var peekId: String?
    @State private var openId: String?

    var body: some View {
        if !store.order.isEmpty {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(store.order, id: \.self) { id in
                        if let entry = store.entry(id) { chip(id, entry) }
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 6)
            }
            .overlay(alignment: .bottomLeading) {
                if let id = peekId, let entry = store.entry(id) {
                    GenerationProgressView(callId: id, lane: entry.detail.lane, mode: .full)
                        .frame(width: 280)
                        .shadow(color: .black.opacity(0.18), radius: 14, y: 6)
                        .padding(.leading, 14)
                        .offset(y: -44)
                        .allowsHitTesting(false)
                        .transition(.opacity)
                }
            }
            .sheet(item: Binding(get: { openId.map(StripId.init) }, set: { openId = $0?.id })) { item in
                GenerationSheet(callId: item.id, lane: store.entry(item.id)?.detail.lane ?? .picture)
            }
        }
    }

    private struct StripId: Identifiable { let id: String }

    private func chip(_ id: String, _ entry: GenerationProgressStore.Entry) -> some View {
        let line = GenerationProgressDetail.mainLine(entry.detail, lane: entry.detail.lane)
        return HStack(spacing: 6) {
            Image(systemName: entry.detail.lane == .picture ? "photo" : "film")
                .font(.system(size: 11, weight: .semibold))
                .accessibilityHidden(true)
            Text(line)
                .font(.system(size: 12))
                .lineLimit(1)
            if let f = GenerationProgressDetail.barFraction(entry.detail) {
                Text("\(Int((f * 100).rounded()))%")
                    .font(.system(size: 12).monospacedDigit())
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .foregroundColor(Color("TextPrimary"))
        .padding(.horizontal, 10)
        .frame(height: 28)
        .background(Capsule().fill(Color("BgSecondary")))
        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        .contentShape(Capsule())
        .onTapGesture { openId = id }
        .onLongPressGesture(minimumDuration: GenerationPeekModifier.holdSeconds, maximumDistance: 40, perform: {
            peekId = id
            Haptics.light()
        }, onPressingChanged: { pressing in
            if !pressing, peekId == id { peekId = nil }
        })
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(entry.detail.lane == .picture ? "Making a picture" : "Making a clip")
        .accessibilityValue(line)
        .accessibilityAddTraits(.isButton)
        .accessibilityHint("Opens the full view")
    }
}
