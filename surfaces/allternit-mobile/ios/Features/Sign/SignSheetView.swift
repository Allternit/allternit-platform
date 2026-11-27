import PencilKit
import PhotosUI
import SwiftUI
import UIKit

/// What the signer produced. The sheet never uploads; the caller (PDFSignView,
/// the floor card) composes the PDF and calls `SignRequestsClient.sign`.
struct SignSheetResult {
    /// Trimmed PNG, at most `SignRequestsClient.maxSignatureBytes`.
    let png: Data
    let image: UIImage
    let method: SignMethod
    /// Where the ink sat on screen, global (window) coordinates: the start rect
    /// for `SignFlight`.
    let sourceRect: CGRect
}

/// Review & sign sheet (web `after.tsx`): a review step, then a sign step with
/// Draw | Type, a PencilKit pad on the neutral fill, an Undo chip, a small
/// "Upload image" link, the consent line and a black Sign & send pill that is
/// disabled until there is ink. Present it from the caller:
///
///     .sheet(isPresented: $showSign) {
///         SignSheetView(title: "Design services", summary: "…", details: rows,
///                       sendTo: "Northside Studio") { result in … } onClose: { … }
///             .signSheetPresentation()
///     }
///
/// `onSign` fires after the "Signed" write-on finishes; dismiss the sheet there
/// and start `SignFlight` from `result.sourceRect`. No new rail tab.
struct SignSheetView: View {
    enum Mode {
        /// A pending record someone else asked for.
        case request
        /// The user signs their own document (a record is created).
        case selfSign
        /// Signed out / offline: places the image only, no record.
        case local
    }

    let title: String
    var summary: String? = nil
    var details: [SignDetail] = []
    /// Shown as "Send signed copy to".
    var sendTo: String? = nil
    var mode: Mode = .request
    var onSign: (SignSheetResult) -> Void
    var onClose: () -> Void

    private enum Step { case review, sign }
    private enum Tab: Hashable { case draw, type }

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @StateObject private var pad = SignPadModel()
    @State private var step: Step
    @State private var tab: Tab = .draw
    @State private var typed = ""
    @State private var uploaded: UIImage?
    @State private var pickerItem: PhotosPickerItem?
    @State private var writing = false
    @State private var padFrame: CGRect = .zero
    @State private var failure: String?
    @FocusState private var typeFocused: Bool

    private static let warn = Color(hex: "#E0612F")
    private static let script = "SnellRoundhand"

    init(title: String, summary: String? = nil, details: [SignDetail] = [], sendTo: String? = nil, mode: Mode = .request,
         onSign: @escaping (SignSheetResult) -> Void, onClose: @escaping () -> Void) {
        self.title = title
        self.summary = summary
        self.details = details
        self.sendTo = sendTo
        self.mode = mode
        self.onSign = onSign
        self.onClose = onClose
        let bare = details.isEmpty && (summary ?? "").isEmpty && sendTo == nil
        _step = State(initialValue: bare ? .sign : .review)
    }

    private var hasInk: Bool {
        if uploaded != nil { return true }
        return tab == .draw ? !pad.isEmpty : !typed.trimmingCharacters(in: .whitespaces).isEmpty
    }

    private var actionLabel: String { mode == .local ? "Place signature" : "Sign & send" }

    var body: some View {
        VStack(spacing: 0) {
            Group {
                switch step {
                case .review: reviewStep.transition(.opacity.combined(with: .offset(y: 8)))
                case .sign: signStep.transition(.opacity.combined(with: .offset(y: 8)))
                }
            }
            .animation(reduceMotion ? nil : .easeOut(duration: 0.22), value: step)
        }
        .padding(.horizontal, 16)
        .padding(.top, 12)
        .padding(.bottom, 16)
        .frame(maxWidth: .infinity)
        .background(Theme.bgPanel.ignoresSafeArea())
        .onChange(of: pickerItem) { _, item in
            guard let item else { return }
            Task { await loadUpload(item) }
        }
    }

    // MARK: Review step

    private var reviewStep: some View {
        VStack(spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 12)).foregroundStyle(Self.warn)
                Text("Awaiting your signature").font(.system(size: 13, weight: .medium)).foregroundStyle(Self.warn)
            }
            Text(title)
                .font(.system(size: 26, weight: .semibold)).tracking(-0.4)
                .foregroundStyle(Theme.textPrimary)
                .multilineTextAlignment(.center)
                .padding(.top, 2)
            if let summary, !summary.isEmpty {
                Text(summary)
                    .font(.system(size: 14)).foregroundStyle(Theme.textSecondary)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 12)
            }
            let rows = reviewRows
            if !rows.isEmpty {
                VStack(spacing: 0) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { i, row in
                        if i > 0 { Rectangle().fill(Theme.borderSubtle).frame(height: 1) }
                        HStack(alignment: .firstTextBaseline) {
                            Text(row.label).foregroundStyle(Theme.textSecondary)
                            Spacer(minLength: 12)
                            Text(row.value).foregroundStyle(Theme.textPrimary).multilineTextAlignment(.trailing)
                        }
                        .font(.system(size: 14))
                        .padding(.vertical, 12)
                    }
                }
                .padding(.horizontal, 16)
                .background(Theme.bgTertiary, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
                .padding(.top, 12)
            }
            HStack(spacing: 8) {
                pill("Close", filled: false, action: onClose).frame(width: 96)
                pill("Add signature", filled: true) { goSign() }
            }
            .padding(.top, 16)
        }
    }

    private var reviewRows: [SignDetail] {
        var rows = Array(details.prefix(8))
        if let sendTo, !sendTo.isEmpty { rows.append(SignDetail(label: "Send signed copy to", value: sendTo)) }
        return rows
    }

    private func goSign() {
        Haptics.light()
        step = .sign
    }

    // MARK: Sign step

    private var signStep: some View {
        VStack(spacing: 4) {
            Text("Your signature")
                .font(.system(size: 26, weight: .semibold)).tracking(-0.4)
                .foregroundStyle(Theme.textPrimary)
            Text("Draw your name or type it below")
                .font(.system(size: 14)).foregroundStyle(Theme.textSecondary)
            tabs.padding(.top, 12)
            padArea.padding(.top, 8)
            HStack {
                PhotosPicker(selection: $pickerItem, matching: .images) {
                    Text(uploaded == nil ? "Upload image" : "Replace image")
                        .font(.system(size: 12.5)).underline()
                        .foregroundStyle(Theme.textSecondary)
                }
                .disabled(writing)
                if uploaded != nil {
                    Button("Remove") { uploaded = nil; pickerItem = nil }
                        .font(.system(size: 12.5)).foregroundStyle(Theme.textSecondary)
                        .disabled(writing)
                }
                Spacer()
            }
            .padding(.horizontal, 4)
            Text(mode == .local
                 ? "You are signed out, so this places your signature on the document only. No signing record is kept."
                 : SignRequestsClient.consentText)
                .font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 8)
            if let failure {
                Text(failure).font(.system(size: 12.5)).foregroundStyle(Self.warn).padding(.top, 4)
            }
            HStack(spacing: 8) {
                pill("Close", filled: false) { if bareStart { onClose() } else { step = .review } }
                    .frame(width: 96).disabled(writing)
                signButton
            }
            .padding(.top, 12)
        }
    }

    private var bareStart: Bool { details.isEmpty && (summary ?? "").isEmpty && sendTo == nil }

    private var tabs: some View {
        HStack(spacing: 0) {
            ForEach([(Tab.draw, "Draw signature"), (Tab.type, "Type signature")], id: \.1) { t, label in
                Button {
                    guard tab != t, !writing else { return }
                    Haptics.selection()
                    withAnimation(reduceMotion ? nil : .spring(response: 0.28, dampingFraction: 0.85)) { tab = t }
                    uploaded = nil
                    typeFocused = t == .type
                } label: {
                    Text(label)
                        .font(.system(size: 13.5, weight: .medium))
                        .foregroundStyle(tab == t ? Theme.textPrimary : Theme.textSecondary)
                        .frame(maxWidth: .infinity).frame(height: 36)
                        .background {
                            if tab == t {
                                Capsule().fill(Theme.bgPanel).shadow(color: .black.opacity(0.08), radius: 2, y: 1)
                            }
                        }
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(tab == t ? [.isSelected] : [])
            }
        }
        .padding(3)
        .background(Theme.bgTertiary, in: Capsule())
    }

    private var padArea: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 20, style: .continuous).fill(Theme.bgTertiary)
            if let uploaded {
                Image(uiImage: uploaded).resizable().scaledToFit().padding(16)
            } else if tab == .draw {
                SignPadCanvas(model: pad).clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
                    .allowsHitTesting(!writing)
                if pad.isEmpty {
                    Text("Sign here").font(.system(size: 13)).foregroundStyle(Theme.textSecondary).allowsHitTesting(false)
                }
            } else {
                VStack(spacing: 0) {
                    Text(typed.isEmpty ? " " : typed)
                        .font(.custom(Self.script, size: 54))
                        .minimumScaleFactor(0.4).lineLimit(1)
                        .foregroundStyle(Theme.textPrimary)
                        .padding(.horizontal, 12)
                        .frame(maxHeight: .infinity)
                    TextField("Type your full name", text: $typed)
                        .focused($typeFocused)
                        .textContentType(.name)
                        .autocorrectionDisabled()
                        .font(.system(size: 14))
                        .padding(.horizontal, 12).frame(height: 36)
                        .background(Theme.bgPanel, in: Capsule())
                        .padding(.horizontal, 12).padding(.bottom, 12)
                        .disabled(writing)
                }
            }
            if tab == .draw, uploaded == nil, !pad.isEmpty, !writing {
                VStack { HStack { Spacer()
                    Button {
                        Haptics.light()
                        pad.undo()
                    } label: {
                        Text("\u{21B6} Undo").font(.system(size: 12, weight: .medium))
                            .foregroundStyle(Theme.textSecondary)
                            .padding(.horizontal, 10).frame(height: 28)
                            .background(Theme.bgPanel, in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .transition(.scale(scale: 0.9).combined(with: .opacity))
                }; Spacer() }.padding(10)
            }
        }
        .frame(height: 168)
        .animation(reduceMotion ? nil : .easeOut(duration: 0.18), value: pad.isEmpty)
        .background(GeometryReader { g in
            Color.clear.onAppear { padFrame = g.frame(in: .global) }
                .onChange(of: g.frame(in: .global)) { _, f in padFrame = f }
        })
    }

    private var signButton: some View {
        Button(action: signAndSend) {
            ZStack {
                if writing {
                    SignedWriteOn(reduceMotion: reduceMotion)
                        .frame(height: 32)
                        .accessibilityLabel("Signed")
                } else {
                    Text(actionLabel)
                }
            }
            .font(.system(size: 15, weight: .medium))
            .foregroundStyle(Theme.bgPanel)
            .frame(maxWidth: .infinity).frame(height: 44)
            .background(Theme.textPrimary.opacity(hasInk ? 1 : 0.18), in: Capsule())
            .animation(reduceMotion ? nil : .easeOut(duration: 0.25), value: hasInk)
        }
        .buttonStyle(.plain)
        .allowsHitTesting(hasInk && !writing)
        .accessibilityRespondsToUserInteraction(hasInk && !writing)
    }

    private func pill(_ label: String, filled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(filled ? Theme.bgPanel : Theme.textPrimary)
                .frame(maxWidth: .infinity).frame(height: 44)
                .background(filled ? Theme.textPrimary : Theme.bgTertiary, in: Capsule())
        }
        .buttonStyle(.plain)
    }

    // MARK: Sign

    private func signAndSend() {
        guard hasInk, !writing else { return }
        guard let built = buildResult() else {
            failure = "The signature could not be saved. Try again."
            return
        }
        failure = nil
        typeFocused = false
        Haptics.medium()
        writing = true
        Task { @MainActor in
            if !reduceMotion { try? await Task.sleep(nanoseconds: 1_050_000_000) }
            onSign(built)
        }
    }

    private func buildResult() -> SignSheetResult? {
        let method: SignMethod
        let image: UIImage
        var rect = padFrame
        if let uploaded {
            method = .upload
            guard let scaled = SignInk.scaled(uploaded, maxSide: 800) else { return nil }
            image = scaled
            rect = SignFlight.fit(image.size, in: padFrame.insetBy(dx: 16, dy: 16))
        } else if tab == .draw {
            method = .draw
            guard let drawn = pad.trimmedImage() else { return nil }
            image = drawn.image
            rect = drawn.globalRect ?? padFrame
        } else {
            method = .type
            guard let text = SignInk.typedImage(typed.trimmingCharacters(in: .whitespaces), fontName: Self.script) else { return nil }
            image = text
            rect = SignFlight.fit(image.size, in: padFrame.insetBy(dx: 12, dy: 12))
        }
        guard let png = SignInk.png(image, maxBytes: SignRequestsClient.maxSignatureBytes),
              let final = UIImage(data: png) else { return nil }
        return SignSheetResult(png: png, image: final, method: method, sourceRect: rect)
    }

    private func loadUpload(_ item: PhotosPickerItem) async {
        guard let data = try? await item.loadTransferable(type: Data.self), let img = UIImage(data: data) else {
            failure = "That image could not be opened."
            return
        }
        failure = nil
        uploaded = img
    }
}

// MARK: - Presentation

extension View {
    /// Detents and chrome for `SignSheetView` in a `.sheet`.
    func signSheetPresentation() -> some View {
        self
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.visible)
            .presentationCornerRadius(34)
    }
}

// MARK: - "Signed" write-on

/// "Signed" in script, revealed left to right by a growing mask (~0.85 s).
private struct SignedWriteOn: View {
    let reduceMotion: Bool
    @State private var progress: CGFloat = 0

    var body: some View {
        Text("Signed")
            .font(.custom("SnellRoundhand", size: 24))
            .foregroundStyle(Theme.bgPanel)
            .mask(alignment: .leading) {
                GeometryReader { g in
                    Rectangle().frame(width: g.size.width * progress)
                }
            }
            .onAppear {
                if reduceMotion { progress = 1 } else { withAnimation(.timingCurve(0.4, 0, 0.6, 1, duration: 0.85)) { progress = 1 } }
            }
    }
}

// MARK: - Pad

@MainActor
final class SignPadModel: ObservableObject {
    let canvas = PKCanvasView()
    @Published var isEmpty = true
    private var observer: Observer?

    init() {
        canvas.backgroundColor = .clear
        canvas.isOpaque = false
        canvas.drawingPolicy = .anyInput
        canvas.tool = PKInkingTool(.pen, color: .black, width: 3)
        let observer = Observer(model: self)
        self.observer = observer
        canvas.delegate = observer
    }

    func undo() {
        var drawing = canvas.drawing
        guard !drawing.strokes.isEmpty else { return }
        drawing.strokes.removeLast()
        canvas.drawing = drawing
        isEmpty = drawing.strokes.isEmpty
    }

    /// Ink only, black on transparent, padded 6 pt, plus where it sits on screen.
    func trimmedImage() -> (image: UIImage, globalRect: CGRect?)? {
        let drawing = canvas.drawing
        guard !drawing.bounds.isEmpty else { return nil }
        let bounds = drawing.bounds.insetBy(dx: -6, dy: -6)
        var image = UIImage()
        UITraitCollection(userInterfaceStyle: .light).performAsCurrent {
            image = drawing.image(from: bounds, scale: 3)
        }
        let global = canvas.window == nil ? nil : canvas.convert(bounds, to: nil)
        return (image, global)
    }

    private final class Observer: NSObject, PKCanvasViewDelegate {
        weak var model: SignPadModel?
        init(model: SignPadModel) { self.model = model }
        func canvasViewDrawingDidChange(_ canvasView: PKCanvasView) {
            let empty = canvasView.drawing.strokes.isEmpty
            let model = self.model
            Task { @MainActor in model?.isEmpty = empty }
        }
    }
}

private struct SignPadCanvas: UIViewRepresentable {
    let model: SignPadModel

    func makeUIView(context: Context) -> PKCanvasView { model.canvas }
    func updateUIView(_ uiView: PKCanvasView, context: Context) {}
}

// MARK: - Ink images

enum SignInk {
    /// Typed name in the script font, black on transparent, trimmed to the text.
    static func typedImage(_ text: String, fontName: String) -> UIImage? {
        guard !text.isEmpty else { return nil }
        let font = UIFont(name: fontName, size: 54) ?? UIFont.italicSystemFont(ofSize: 54)
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: UIColor.black]
        let size = (text as NSString).size(withAttributes: attributes)
        let pad: CGFloat = 8
        let canvas = CGSize(width: ceil(size.width) + pad * 2, height: ceil(size.height) + pad * 2)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 3
        format.opaque = false
        return UIGraphicsImageRenderer(size: canvas, format: format).image { _ in
            (text as NSString).draw(at: CGPoint(x: pad, y: pad), withAttributes: attributes)
        }
    }

    /// Down-scales so the longest side is at most `maxSide` points.
    static func scaled(_ image: UIImage, maxSide: CGFloat) -> UIImage? {
        let longest = max(image.size.width, image.size.height)
        guard longest > 0 else { return nil }
        let k = min(1, maxSide / longest)
        let size = CGSize(width: image.size.width * k, height: image.size.height * k)
        let format = UIGraphicsImageRendererFormat()
        format.scale = image.scale
        format.opaque = false
        return UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }
    }

    /// PNG that fits `maxBytes`, shrinking in steps when it does not.
    static func png(_ image: UIImage, maxBytes: Int) -> Data? {
        var current = image
        for _ in 0..<6 {
            if let data = current.pngData(), data.count <= maxBytes { return data }
            let longest = max(current.size.width, current.size.height)
            guard let smaller = scaled(current, maxSide: longest * 0.7) else { return nil }
            current = smaller
        }
        return nil
    }
}
