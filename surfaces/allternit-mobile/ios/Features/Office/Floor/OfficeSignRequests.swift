import SwiftUI

/// Pending sign requests on the Office floor. "Awaiting your signature" cards
/// in the approval card style; tap opens the document in `PDFSignView` with the
/// request, the card's menu declines. Signed out: no cards, no error. The
/// frame/agent never signs: only the person, in `PDFSignView`.
@MainActor
final class OfficeSignRequestsModel: ObservableObject {
    @Published private(set) var pending: [SignRequest] = []
    @Published var opening: String?
    @Published var error: String?
    @Published var presented: OfficeSignDocument?

    func refresh(signedIn: Bool) async {
        guard signedIn else {
            pending = []
            return
        }
        // A failed list keeps what is shown; the floor never shows an error for it.
        if let list = try? await SignRequestsClient.shared.list(status: .pending, limit: 10) {
            pending = list.filter { $0.status == .pending }
        }
    }

    func open(_ request: SignRequest) async {
        guard opening == nil else { return }
        guard let raw = request.documentUrl, let url = URL(string: raw) else {
            error = "This document isn't available to open right now."
            return
        }
        opening = request.id
        error = nil
        defer { opening = nil }
        do {
            let (tmp, _) = try await URLSession.shared.download(from: url)
            let dest = FileManager.default.temporaryDirectory
                .appendingPathComponent("sign-\(UUID().uuidString)-\(Self.safeName(request.documentName))")
            try? FileManager.default.removeItem(at: dest)
            try FileManager.default.moveItem(at: tmp, to: dest)
            presented = OfficeSignDocument(url: dest, request: request)
        } catch {
            self.error = "Couldn't open the document. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    func decline(_ request: SignRequest, signedIn: Bool) async {
        error = nil
        do {
            _ = try await SignRequestsClient.shared.decline(request.id, reason: nil)
            Haptics.success()
        } catch {
            self.error = "Couldn't decline. \(FriendlyErrorMessage.from(error.localizedDescription))"
            Haptics.error()
        }
        await refresh(signedIn: signedIn)
    }

    private static func safeName(_ name: String) -> String {
        let cleaned = name.replacingOccurrences(of: "/", with: "-")
        return cleaned.lowercased().hasSuffix(".pdf") ? cleaned : cleaned + ".pdf"
    }
}

struct OfficeSignDocument: Identifiable {
    let id = UUID()
    let url: URL
    let request: SignRequest
}

struct OfficeSignCards: View {
    @ObservedObject var model: OfficeSignRequestsModel
    var signedIn: Bool

    var body: some View {
        VStack(spacing: 8) {
            ForEach(model.pending.prefix(3)) { request in
                OfficeSignCard(request: request, busy: model.opening == request.id,
                               onOpen: { Task { await model.open(request) } },
                               onDecline: { Task { await model.decline(request, signedIn: signedIn) } })
            }
            if let error = model.error {
                Text(error)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .sheet(item: $model.presented, onDismiss: { Task { await model.refresh(signedIn: signedIn) } }) { doc in
            PDFSignView(sourceURL: doc.url, request: doc.request)
        }
    }
}

struct OfficeSignCard: View {
    let request: SignRequest
    var busy: Bool
    var onOpen: () -> Void
    var onDecline: () -> Void

    private static let warn = Color(hex: "#E0612F")

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Button(action: onOpen) {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 6) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(.caption)
                            .accessibilityHidden(true)
                        Text("Awaiting your signature")
                            .font(.caption.weight(.semibold))
                    }
                    .foregroundColor(Self.warn)

                    Text(request.title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .multilineTextAlignment(.leading)
                        .lineLimit(2)
                    Text(subtitle)
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(busy)

            if busy {
                ProgressView().controlSize(.small)
            } else {
                Menu {
                    Button("Decline", role: .destructive, action: onDecline)
                } label: {
                    Image(systemName: "ellipsis")
                        .font(.body.weight(.semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 32, height: 32)
                        .contentShape(Rectangle())
                }
                .accessibilityLabel("More for \(request.title)")
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Awaiting your signature: \(request.title)")
    }

    private var subtitle: String {
        let who = request.requesterName.flatMap { $0.isEmpty ? nil : $0 } ?? "A bot"
        guard let when = Self.relative(request.createdAt) else { return who }
        return "\(who) · \(when)"
    }

    private static func relative(_ iso: String) -> String? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let g = ISO8601DateFormatter()
        guard let date = f.date(from: iso) ?? g.date(from: iso) else { return nil }
        let r = RelativeDateTimeFormatter()
        r.unitsStyle = .short
        return r.localizedString(for: date, relativeTo: Date())
    }
}
