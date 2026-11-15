import SwiftUI
import UIKit

/// Where the Review pane gets its data. `live` goes through the relay;
/// the debug gallery passes fixtures.
struct PhoneHoldSource: Sendable {
    var crop: @Sendable (_ holdId: String) async -> UIImage?
    var state: @Sendable (_ holdId: String) async -> PhoneHoldState?
    var approve: @Sendable (InboxItem) async throws -> Void
    var deny: @Sendable (InboxItem) async throws -> Void

    static let live = PhoneHoldSource(
        crop: { await PhoneTaskClient.shared.crop(holdId: $0) },
        state: { try? await PhoneTaskClient.shared.hold(id: $0) },
        approve: { try await InboxClient.shared.approveReviewed($0.id, contentHash: $0.contentHash) },
        deny: { try await InboxClient.shared.deny($0.id) }
    )
}

/// The phone approval card, Option B (Eoj, 2026-11-10): the inbox row shows
/// the hold; Review opens this focused pane with the large screen image.
/// Approve turns on only after the image has loaded; Deny always works.
/// States: crop loading, crop shown, crop failed, approved, denied,
/// changed, expired (the row itself is "waiting").
struct PhoneHoldReviewView: View {
    enum Phase: Equatable {
        case loading, shown, failed, approved, denied, changed, expired
    }

    let item: InboxItem
    var source: PhoneHoldSource = .live
    /// Debug gallery: show one phase and skip loading.
    var fixedPhase: Phase? = nil
    var fixedImage: UIImage? = nil
    var cropHeight: CGFloat = 380
    var onClose: (() -> Void)? = nil

    @State private var phase: Phase = .loading
    @State private var image: UIImage?
    @State private var busy = false
    @State private var error: String?

    private var phone: PhoneHoldDetail? { item.detail?.phone }
    private var phoneName: String { phone?.phoneName?.nilIfEmpty ?? "your phone" }
    private var actionLabel: String {
        phone?.element?.label?.nilIfEmpty ?? phone?.intent?.nilIfEmpty ?? phone?.tool ?? "this step"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            screen
            details
            outcomeOrButtons
        }
        .padding(16)
        .background(Color("BgPrimary"))
        .task(id: item.id) { await load() }
    }

    // MARK: Parts

    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("APPROVAL · PHONE")
                .font(.system(size: 11, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
            Text(item.summary)
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            Text([phoneName, phone?.app].compactMap { $0?.nilIfEmpty }.joined(separator: " · "))
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var screen: some View {
        let shape = RoundedRectangle(cornerRadius: 16, style: .continuous)
        ZStack {
            shape.fill(Color("FillNeutral"))
            switch currentPhase {
            case .loading:
                VStack(spacing: 8) {
                    ProgressView()
                    Text("Loading the phone's screen…")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                }
            case .failed:
                VStack(spacing: 10) {
                    Image(systemName: "photo.badge.exclamationmark")
                        .font(.system(size: 26))
                        .foregroundColor(Color("TextSecondary"))
                    Text("Could not load the screen. Approve stays off until you can see it.")
                        .font(.footnote)
                        .multilineTextAlignment(.center)
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 24)
                    Button("Try again") { Task { await loadCrop() } }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .disabled(fixedPhase != nil)
                }
            default:
                if let shown = image ?? fixedImage {
                    Image(uiImage: shown)
                        .resizable()
                        .scaledToFit()
                        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                        .padding(8)
                        .accessibilityLabel("Screenshot of \(phoneName) at this step")
                }
            }
        }
        .frame(maxWidth: .infinity)
        .frame(height: cropHeight)
        .overlay(shape.stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private var details: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let goal = phone?.goal?.nilIfEmpty { detailRow("Goal", goal) }
            if let to = phone?.to?.nilIfEmpty { detailRow("To", to) }
            if let text = phone?.text?.nilIfEmpty { detailRow("Message", text) }
            detailRow("Action", actionLabel)
        }
    }

    private func detailRow(_ title: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(title)
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 64, alignment: .leading)
            Text(value)
                .font(.system(size: 13))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder
    private var outcomeOrButtons: some View {
        switch currentPhase {
        case .approved:
            outcome("checkmark.circle.fill", .green, "Approved. Your bot taps \"\(actionLabel)\" now.")
        case .denied:
            outcome("xmark.circle.fill", .red, "Denied. The task stopped and nothing was done on your phone.")
        case .expired:
            outcome("clock.badge.xmark", Color("TextSecondary"), "Expired. Nobody answered in 5 minutes, so the task ended and nothing was done.")
        case .changed:
            VStack(alignment: .leading, spacing: 10) {
                outcome("arrow.triangle.2.circlepath", .orange, "Changed. The screen moved since your bot asked. Check the new screen, then decide.")
                buttons(approveEnabled: false)
            }
        default:
            VStack(alignment: .leading, spacing: 8) {
                if currentPhase != .shown {
                    Text("Approve turns on when the screen has loaded.")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                }
                if let error {
                    Text(error).font(.footnote).foregroundColor(.red)
                }
                buttons(approveEnabled: currentPhase == .shown)
            }
        }
    }

    private func outcome(_ icon: String, _ tint: Color, _ text: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: icon).foregroundColor(tint)
            Text(text)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private func buttons(approveEnabled: Bool) -> some View {
        HStack(spacing: 10) {
            Button { Task { await decide(approve: false) } } label: {
                Text("Deny")
                    .font(.system(size: 15, weight: .semibold))
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .foregroundColor(Color("TextPrimary"))
                    .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle")))
            }
            .buttonStyle(.plain)
            .disabled(busy)
            // Ink on paper in both themes: the old white-on-tan Approve was too
            // faint in graphite dark.
            Button { Task { await decide(approve: true) } } label: {
                Text(busy ? "Working…" : "Approve")
                    .font(.system(size: 15, weight: .semibold))
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .foregroundColor(Color("BgPrimary"))
                    .background(Color("TextPrimary").opacity(approveEnabled ? 1 : 0.28),
                                in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            }
            .buttonStyle(.plain)
            .disabled(!approveEnabled || busy)
            .accessibilityHint(approveEnabled ? "" : "Turns on when the screen has loaded")
        }
    }

    // MARK: Loading and deciding

    private var currentPhase: Phase { fixedPhase ?? phase }

    private func load() async {
        guard fixedPhase == nil else { return }
        if let state = await source.state(item.holdId), let done = Self.phase(for: state) {
            phase = done
            if done == .changed { await loadCrop(keepPhase: true) }
            return
        }
        await loadCrop()
    }

    private func loadCrop(keepPhase: Bool = false) async {
        if !keepPhase { phase = .loading }
        let loaded = await source.crop(item.holdId)
        image = loaded
        if !keepPhase { phase = loaded == nil ? .failed : .shown }
    }

    private func decide(approve: Bool) async {
        guard fixedPhase == nil, !busy else { return }
        busy = true
        error = nil
        defer { busy = false }
        do {
            if approve { try await source.approve(item) } else { try await source.deny(item) }
            phase = approve ? .approved : .denied
            InboxStore.shared.removeLocally(item.id)
        } catch APIError.httpError(let status, _) where status == 409 {
            // The hold moved (another screen, or it timed out): ask again.
            if let state = await source.state(item.holdId), let done = Self.phase(for: state) {
                phase = done
            } else {
                phase = .changed
            }
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private static func phase(for state: PhoneHoldState) -> Phase? {
        switch state {
        case .pending: return nil
        case .approved: return .approved
        case .denied: return .denied
        case .changed: return .changed
        case .expired, .gone: return .expired
        }
    }
}

/// The Review pane as a sheet over the inbox.
struct PhoneHoldReviewSheet: View {
    let item: InboxItem
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView { PhoneHoldReviewView(item: item) }
                .background(Color("BgPrimary").ignoresSafeArea())
                .navigationTitle("Review")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } }
                }
        }
        .presentationDetents([.large])
    }
}

private extension String {
    var nilIfEmpty: String? {
        let t = trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }

}
