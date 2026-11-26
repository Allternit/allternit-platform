import SwiftUI
import PDFKit
import UIKit

/// Review & sign for a PDF: the document, a dashed "Sign here" slot, an orange
/// "Awaiting your signature" bar that opens `SignSheetView`, a Preview pill that
/// lowers the sheet so the document shows, then `SignFlight` from the pad to the
/// slot and "Signed <date>" under it (web `after.tsx`, `sign-clip-beats.md`).
///
/// - With a `request` (an agent or app asked for it) the slots come from the
///   record and `/sign` closes it. Without one, a signed-in user signs their own
///   document (a record is created first). Signed out, the signature is placed
///   and saved on this device only: no record.
/// - No slot: tap the page to move the default slot (bottom-right of the last page).
struct PDFSignView: View {
    let sourceURL: URL?
    var request: SignRequest? = nil

    @ObservedObject private var auth = AuthManager.shared
    @StateObject private var controller = PDFSignController()
    @Environment(\.dismiss) private var dismiss

    @State private var document: PDFDocument?
    @State private var originalData: Data?
    @State private var loadError: String?
    @State private var slot: SignSlot?
    @State private var showSheet = false
    @State private var detent: PresentationDetent = .medium
    @State private var result: SignSheetResult?
    @State private var flight: FlightPlan?
    @State private var placed = false
    @State private var signedDate: Date?
    @State private var failure: String?
    @State private var submitting = false
    @State private var signedPDF: Data?
    @State private var shareURL: URL?

    private static let peekDetent = PresentationDetent.height(96)
    private static let warn = Color(hex: "#E0612F")

    private struct FlightPlan: Equatable {
        let image: UIImage
        let from: CGRect
        let to: CGRect
    }

    private var hasRequest: Bool { request != nil }
    private var isPending: Bool { request.map { $0.status == .pending } ?? true }
    private var documentName: String { request?.documentName ?? sourceURL?.lastPathComponent ?? "Document.pdf" }
    private var mode: SignSheetView.Mode {
        if hasRequest { return .request }
        return auth.isSignedIn ? .selfSign : .local
    }
    private var canPlace: Bool { (request?.slots.isEmpty ?? true) && !placed && flight == nil && !submitting }

    var body: some View {
        NavigationStack {
            ZStack(alignment: .bottom) {
                content
                bottomBar
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle(request?.title ?? "Sign PDF")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button(placed ? "Done" : "Cancel") { dismiss() }
                }
                if placed, signedPDF != nil {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button { shareSigned() } label: { Label("Share", systemImage: "square.and.arrow.up") }
                    }
                }
            }
            .overlay { previewPill }
            .sheet(isPresented: $showSheet) { signSheet }
            .sheet(isPresented: Binding(get: { shareURL != nil }, set: { if !$0 { shareURL = nil } })) {
                if let url = shareURL { ArtifactShareSheet(items: [url]) }
            }
        }
        .overlay {
            if let flight {
                SignFlight(image: flight.image, from: flight.from, to: flight.to) {
                    finishFlight()
                }
                .ignoresSafeArea()
            }
        }
        .task { await loadDocument() }
    }

    // MARK: Content

    @ViewBuilder
    private var content: some View {
        if let document {
            PDFSlotView(document: document, controller: controller, slot: slot,
                        ink: placed ? result?.image : nil,
                        signedLabel: placed ? signedDate.map { "Signed \(Self.shortDate($0))" } : nil,
                        placing: canPlace) { moved in slot = moved }
                .opacity(showSheet && detent == .large ? 0.5 : 1)
        } else if let loadError {
            FriendlyStateView(style: .empty, icon: "doc.text", title: "Couldn't open the PDF", message: loadError)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ProgressView("Loading document…").frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // MARK: Bottom bar

    @ViewBuilder
    private var bottomBar: some View {
        VStack(spacing: 8) {
            if let failure {
                Text(failure)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Self.warn)
                    .multilineTextAlignment(.leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 20)
                    .accessibilityIdentifier("sign-pdf-error")
            }
            if document != nil, !placed, !showSheet {
                if isPending {
                    peekBar
                } else if let request {
                    Text("This request is \(request.status.rawValue).")
                        .font(.system(size: 13.5, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: .infinity).frame(height: 56)
                }
            } else if placed {
                Text(mode == .local ? "Placed on this device. No signing record was made."
                                    : "Signed and saved to your files.")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity).frame(height: 44)
            }
        }
        .padding(.top, 8)
        .background(
            Color("BgPrimary")
                .clipShape(UnevenRoundedRectangle(topLeadingRadius: 20, topTrailingRadius: 20))
                .shadow(color: .black.opacity(0.08), radius: 10, y: -2)
                .ignoresSafeArea(edges: .bottom)
        )
    }

    private var peekBar: some View {
        Button { openSheet() } label: {
            HStack(spacing: 6) {
                Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 13))
                Text("Awaiting your signature").font(.system(size: 13.5, weight: .medium))
            }
            .foregroundColor(Self.warn)
            .frame(maxWidth: .infinity).frame(height: 48)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(submitting)
        .accessibilityLabel("Awaiting your signature")
        .accessibilityHint("Opens the signing sheet")
    }

    // MARK: Preview pill (sits above the sheet)

    @ViewBuilder
    private var previewPill: some View {
        if showSheet && detent != .large {
            GeometryReader { geo in
                let peeking = detent == Self.peekDetent
                let sheetHeight = peeking ? 96 : geo.size.height * 0.5
                VStack {
                    Spacer()
                    Button {
                        withAnimation(.spring(response: 0.4, dampingFraction: 0.85)) {
                            detent = peeking ? .medium : Self.peekDetent
                        }
                    } label: {
                        HStack(spacing: 6) {
                            Image(systemName: peeking ? "pencil.and.scribble" : "eye").font(.system(size: 12))
                            Text(peeking ? "Back to signing" : "Preview").font(.system(size: 12.5, weight: .medium))
                        }
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 12).frame(height: 32)
                        .background(Capsule().fill(Color("BgPrimary")))
                        .shadow(color: .black.opacity(0.14), radius: 5, y: 2)
                    }
                    .buttonStyle(.plain)
                    .padding(.bottom, sheetHeight + 12)
                }
                .frame(maxWidth: .infinity)
            }
            .transition(.opacity)
        }
    }

    // MARK: Sheet

    private var signSheet: some View {
        SignSheetView(
            title: request?.title ?? documentName,
            summary: request?.summary,
            details: request?.details ?? [],
            sendTo: request?.sendTo.map { $0.name ?? $0.email },
            mode: mode,
            onSign: { signed in handleSigned(signed) },
            onClose: { showSheet = false }
        )
        .presentationDetents([Self.peekDetent, .medium, .large], selection: $detent)
        .presentationDragIndicator(.visible)
        .presentationCornerRadius(34)
        .presentationBackgroundInteraction(.enabled(upThrough: .medium))
    }

    private func openSheet() {
        failure = nil
        detent = .medium
        showSheet = true
    }

    // MARK: After the sheet

    private func handleSigned(_ signed: SignSheetResult) {
        result = signed
        signedDate = Date()
        showSheet = false
        guard let slot else { return }
        controller.reveal(slot)
        let from = signed.sourceRect
        submitting = true
        Task { @MainActor in
            // Let the sheet slide away, then lift the ink out of where the pad was.
            try? await Task.sleep(nanoseconds: 350_000_000)
            let to = controller.globalRect(for: slot) ?? CGRect(x: from.minX, y: from.minY - 200, width: 180, height: 56)
            flight = FlightPlan(image: signed.image, from: from, to: to)
        }
        Task { await submit(signed, slot: slot) }
    }

    private func finishFlight() {
        flight = nil
        placed = failure == nil
    }

    // MARK: Submit

    private func submit(_ signed: SignSheetResult, slot: SignSlot) async {
        defer { submitting = false }
        do {
            guard let document, let original = originalData ?? document.dataRepresentation() else {
                throw SubmitFailure.message("The document could not be read.")
            }
            let name = auth.displayName
            let now = signedDate ?? Date()
            switch mode {
            case .local:
                let data = try Self.compose(document, image: signed.image, slot: slot,
                                            footer: "Signed on this device by \(name) · \(Self.stamp(now))")
                signedPDF = data
                saveSignedPDFToLibrary(data: data)
            case .request, .selfSign:
                let client = SignRequestsClient.self
                let originalHash = client.sha256Hex(original)
                var record = request
                if record == nil {
                    let uploadedOriginal = try await CloudFilesClient.shared.upload(original, name: documentName, contentType: "application/pdf")
                    let draft = SignRequestDraft(title: documentName, documentFileId: uploadedOriginal.fileId,
                                                 documentSha256: originalHash, slots: [slot], requesterKind: "self",
                                                 sourceSurface: "ios")
                    record = try await SignRequestsClient.shared.create(draft)
                }
                guard let record else { throw SubmitFailure.message("The signing record could not be created.") }
                let id8 = String(record.id.prefix(8))
                let data = try Self.compose(document, image: signed.image, slot: slot,
                                            footer: "Signed electronically by \(name) · \(Self.stamp(now)) · \(id8)")
                let signedName = Self.signedName(documentName)
                let uploaded = try await CloudFilesClient.shared.upload(data, name: signedName, contentType: "application/pdf")
                let submission = SignSubmission(method: signed.method,
                                                signaturePngBase64: signed.png.base64EncodedString(),
                                                slot: slot, documentSha256: originalHash,
                                                signedFileId: uploaded.fileId, signedSha256: client.sha256Hex(data))
                _ = try await SignRequestsClient.shared.sign(record.id, submission)
                signedPDF = data
                saveSignedPDFToLibrary(data: data)
            }
        } catch {
            failure = Self.message(for: error)
            placed = false
            flight = nil
            signedDate = nil
        }
    }

    private enum SubmitFailure: LocalizedError {
        case message(String)
        var errorDescription: String? { if case .message(let m) = self { return m }; return nil }
    }

    private static func message(for error: Error) -> String {
        if let e = error as? SignRequestsClient.SignError {
            if e.isNotPending { return "This request was already signed or closed. Nothing was changed." }
            if e.isDocumentChanged { return "The document changed after it was sent. Ask for a new request." }
            if e.isExpired { return "This request has expired. Ask for a new one." }
            return e.message
        }
        if let e = error as? CloudFilesClient.UploadError { return e.message }
        return error.localizedDescription
    }

    // MARK: Compose (vector pages + the signature in the slot + footer line)

    static func compose(_ document: PDFDocument, image: UIImage, slot: SignSlot, footer: String) throws -> Data {
        guard document.pageCount > 0, let first = document.page(at: 0) else {
            throw SubmitFailure.message("The document has no pages.")
        }
        let renderer = UIGraphicsPDFRenderer(bounds: CGRect(origin: .zero, size: first.bounds(for: .mediaBox).size))
        let data = renderer.pdfData { ctx in
            for index in 0..<document.pageCount {
                guard let page = document.page(at: index) else { continue }
                let b = page.bounds(for: .mediaBox)
                ctx.beginPage(withBounds: CGRect(origin: .zero, size: b.size), pageInfo: [:])
                let cg = ctx.cgContext
                cg.saveGState()
                cg.translateBy(x: -b.minX, y: b.maxY)
                cg.scaleBy(x: 1, y: -1)
                page.draw(with: .mediaBox, to: cg)
                cg.restoreGState()
                guard index == slot.page else { continue }
                // PDF points (origin bottom-left) -> UIKit page space (top-left).
                let rect = CGRect(x: slot.x - b.minX, y: b.maxY - slot.y - slot.h, width: slot.w, height: slot.h)
                image.draw(in: SignFlight.fit(image.size, in: rect))
                let attrs: [NSAttributedString.Key: Any] = [
                    .font: UIFont.systemFont(ofSize: 7),
                    .foregroundColor: UIColor(white: 0.25, alpha: 1)
                ]
                let lineY = rect.maxY + 2 + 9 <= b.height ? rect.maxY + 2 : max(rect.minY - 10, 0)
                (footer as NSString).draw(at: CGPoint(x: rect.minX, y: lineY), withAttributes: attrs)
            }
        }
        return data
    }

    // MARK: Local save (best effort, as before)

    private func saveSignedPDFToLibrary(data: Data) {
        let fileName = "signed-\(UUID().uuidString).pdf"
        guard let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first else { return }
        let fileURL = docs.appendingPathComponent(fileName)
        do {
            try data.write(to: fileURL, options: .atomic)
            let record = ArtifactRecord(
                id: fileName,
                title: sourceURL?.lastPathComponent ?? request?.documentName ?? "Signed PDF",
                fileType: "pdf",
                artifactId: fileName,
                artifactType: "pdf",
                url: fileURL.absoluteString,
                inlinePreview: "Signed PDF — \(Int(Date().timeIntervalSince1970))"
            )
            ArtifactLibraryStore.shared.record(record, sessionId: nil)
        } catch {
            // Best-effort local persistence; the signed record is already saved.
        }
    }

    private func shareSigned() {
        guard let signedPDF else { return }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(Self.signedName(documentName))
        try? signedPDF.write(to: url, options: .atomic)
        shareURL = url
    }

    // MARK: Load

    private func loadDocument() async {
        guard document == nil else { return }
        var data: Data?
        var doc: PDFDocument?
        if let url = sourceURL {
            data = try? Data(contentsOf: url)
            doc = data.flatMap { PDFDocument(data: $0) } ?? PDFDocument(url: url)
        } else if let link = request?.documentUrl, let url = URL(string: link) {
            if let (body, _) = try? await URLSession.shared.data(from: url) {
                data = body
                doc = PDFDocument(data: body)
            }
        } else {
            doc = Self.blankPDF()
        }
        guard let doc, doc.pageCount > 0 else {
            loadError = "The PDF could not be loaded."
            return
        }
        document = doc
        originalData = data ?? doc.dataRepresentation()
        slot = request?.slots.first ?? Self.defaultSlot(in: doc)
    }

    static func defaultSlot(in document: PDFDocument) -> SignSlot? {
        let index = max(document.pageCount - 1, 0)
        guard let page = document.page(at: index) else { return nil }
        let b = page.bounds(for: .mediaBox)
        let w = 180.0, h = 56.0, margin = 36.0
        return SignSlot(id: "slot-1", label: "Signature", page: index,
                        x: Double(b.maxX) - w - margin, y: Double(b.minY) + margin, w: w, h: h)
    }

    private static func blankPDF() -> PDFDocument {
        let size = CGSize(width: 612, height: 792)
        let data = UIGraphicsPDFRenderer(bounds: CGRect(origin: .zero, size: size)).pdfData { $0.beginPage() }
        return PDFDocument(data: data) ?? PDFDocument()
    }

    // MARK: Formatting

    private static func shortDate(_ date: Date) -> String {
        let f = DateFormatter()
        f.dateFormat = "d MMM yyyy"
        return f.string(from: date)
    }

    private static func stamp(_ date: Date) -> String {
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd HH:mm zzz"
        return f.string(from: date)
    }

    private static func signedName(_ name: String) -> String {
        let base = (name as NSString).deletingPathExtension
        return "\(base.isEmpty ? "Document" : base)-signed.pdf"
    }
}

// MARK: - PDF view with the slot

@MainActor
final class PDFSignController: ObservableObject {
    weak var pdfView: PDFView?

    /// The slot in global (window) coordinates, for `SignFlight`.
    func globalRect(for slot: SignSlot) -> CGRect? {
        guard let view = pdfView, let doc = view.document, let page = doc.page(at: slot.page) else { return nil }
        let inView = view.convert(CGRect(x: slot.x, y: slot.y, width: slot.w, height: slot.h), from: page)
        return view.convert(inView, to: nil)
    }

    /// Scrolls the slot into view.
    func reveal(_ slot: SignSlot) {
        guard let view = pdfView, let page = view.document?.page(at: slot.page) else { return }
        view.go(to: CGRect(x: slot.x, y: slot.y, width: slot.w, height: slot.h + 80), on: page)
    }
}

private struct PDFSlotView: UIViewRepresentable {
    let document: PDFDocument
    let controller: PDFSignController
    let slot: SignSlot?
    let ink: UIImage?
    let signedLabel: String?
    let placing: Bool
    let onPlace: (SignSlot) -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> PDFView {
        let view = PDFView()
        view.document = document
        view.autoScales = true
        view.backgroundColor = .clear
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.tapped(_:)))
        tap.cancelsTouchesInView = false
        tap.delegate = context.coordinator
        view.addGestureRecognizer(tap)
        context.coordinator.view = view
        controller.pdfView = view
        return view
    }

    func updateUIView(_ view: PDFView, context: Context) {
        let c = context.coordinator
        c.placing = placing
        c.onPlace = onPlace
        c.current = slot
        controller.pdfView = view
        guard c.rendered?.matches(slot: slot, ink: ink, label: signedLabel) != true else { return }
        if let old = c.annotation { old.page?.removeAnnotation(old) }
        c.annotation = nil
        if let slot, let page = document.page(at: slot.page) {
            let note = SignSlotAnnotation(slot: slot, ink: ink, signedLabel: signedLabel)
            page.addAnnotation(note)
            c.annotation = note
        }
        c.rendered = Rendered(slot: slot, ink: ink, label: signedLabel)
        view.setNeedsDisplay()
        view.documentView?.setNeedsDisplay()
    }

    struct Rendered {
        let slot: SignSlot?
        let ink: UIImage?
        let label: String?
        func matches(slot: SignSlot?, ink: UIImage?, label: String?) -> Bool {
            self.slot == slot && self.ink === ink && self.label == label
        }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        weak var view: PDFView?
        var annotation: SignSlotAnnotation?
        var rendered: Rendered?
        var placing = false
        var current: SignSlot?
        var onPlace: (SignSlot) -> Void = { _ in }

        func gestureRecognizer(_ g: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool { true }

        @objc func tapped(_ g: UITapGestureRecognizer) {
            guard placing, let view, let doc = view.document else { return }
            let point = g.location(in: view)
            guard let page = view.page(for: point, nearest: false) else { return }
            let index = doc.index(for: page)
            let p = view.convert(point, to: page)
            let b = page.bounds(for: .mediaBox)
            let w = current?.w ?? 180, h = current?.h ?? 56
            let x = min(max(Double(p.x) - w / 2, Double(b.minX) + 8), Double(b.maxX) - w - 8)
            let y = min(max(Double(p.y) - h / 2, Double(b.minY) + 24), Double(b.maxY) - h - 8)
            onPlace(SignSlot(id: current?.id ?? "slot-1", label: current?.label ?? "Signature",
                             page: index, x: x, y: y, w: w, h: h))
        }
    }
}

/// Dashed "Sign here" box; after signing it holds the ink with "Signed <date>" under it.
private final class SignSlotAnnotation: PDFAnnotation {
    private static let extra: CGFloat = 16
    private var ink: UIImage?
    private var signedLabel: String?
    private var slotHeight: CGFloat = 56

    init(slot: SignSlot, ink: UIImage?, signedLabel: String?) {
        let bounds = CGRect(x: slot.x, y: slot.y - Double(Self.extra), width: slot.w, height: slot.h + Double(Self.extra))
        super.init(bounds: bounds, forType: .square, withProperties: nil)
        self.ink = ink
        self.signedLabel = signedLabel
        self.slotHeight = CGFloat(slot.h)
        self.shouldDisplay = true
        self.shouldPrint = false
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func draw(with box: PDFDisplayBox, in context: CGContext) {
        let b = bounds
        UIGraphicsPushContext(context)
        context.saveGState()
        // Local space: top-left origin, slot on top, the date line below it.
        context.translateBy(x: b.minX, y: b.maxY)
        context.scaleBy(x: 1, y: -1)
        let slotRect = CGRect(x: 0, y: 0, width: b.width, height: slotHeight)
        if let ink {
            ink.draw(in: SignFlight.fit(ink.size, in: slotRect))
            if let signedLabel {
                (signedLabel as NSString).draw(at: CGPoint(x: 0, y: slotHeight + 2), withAttributes: [
                    .font: UIFont.systemFont(ofSize: 8, weight: .medium),
                    .foregroundColor: UIColor.darkGray
                ])
            }
        } else {
            let orange = UIColor(red: 0xE0 / 255, green: 0x61 / 255, blue: 0x2F / 255, alpha: 1)
            let path = UIBezierPath(roundedRect: slotRect.insetBy(dx: 1, dy: 1), cornerRadius: 8)
            path.lineWidth = 1.2
            path.setLineDash([5, 4], count: 2, phase: 0)
            orange.setStroke()
            orange.withAlphaComponent(0.06).setFill()
            path.fill()
            path.stroke()
            let text = "Sign here" as NSString
            let attrs: [NSAttributedString.Key: Any] = [.font: UIFont.systemFont(ofSize: 11, weight: .medium), .foregroundColor: orange]
            let size = text.size(withAttributes: attrs)
            text.draw(at: CGPoint(x: slotRect.midX - size.width / 2, y: slotRect.midY - size.height / 2), withAttributes: attrs)
        }
        context.restoreGState()
        UIGraphicsPopContext()
    }
}
