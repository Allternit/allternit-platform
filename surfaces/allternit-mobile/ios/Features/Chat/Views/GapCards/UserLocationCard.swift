import SwiftUI

/// Location card. While status is "requested" it offers "Share location":
/// the phone's `device.location.get` runs (approximate unless precise was
/// requested) and a short follow-up message ("Here's my location.") carries the
/// fix as the send's client_location field, not in the text.
struct UserLocationCard: View {
    let gap: GapToolMetadata
    var onSend: ((String) -> Void)? = nil
    @State private var sharing = false
    @State private var shareError: String?
    @State private var shared = false

    private var line: String {
        [gap.status, [gap.city, gap.region, gap.country].compactMap { $0 }.joined(separator: ", "), gap.precision].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " \u{00B7} ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "location")
                    .foregroundColor(Color("AccentPrimary"))
                Text("Location")
                    .foregroundColor(Color("TextSecondary"))
            }
            .font(.caption.weight(.semibold))
            if !line.isEmpty {
                Text(line)
                    .font(.footnote)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
            }
            if gap.status == "requested", !shared, onSend != nil {
                Button {
                    Task { await share() }
                } label: {
                    Text(sharing ? "Finding location\u{2026}" : "Share location")
                        .font(.footnote.weight(.semibold))
                }
                .disabled(sharing)
                .accessibilityHint(gap.precision == "precise" ? "Shares your precise location" : "Shares your approximate location")
                if let shareError {
                    Text(shareError).font(.caption).foregroundColor(Color("TextSecondary"))
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .accessibilityElement(children: .contain)
    }

    private func share() async {
        sharing = true
        shareError = nil
        defer { sharing = false }
        do {
            let result = try await DeviceAccessService.handle(
                method: "device.location.get",
                params: ["precision": gap.precision == "precise" ? "precise" : "approximate"])
            let row = result as? [String: Any] ?? [:]
            let precise = (row["precision"] as? String) == "precise"
            guard let lat = row["lat"] as? Double, let lon = row["lon"] as? Double else {
                shareError = "This iPhone could not find its location just now."
                return
            }
            let accuracy = (row["accuracy_m"] as? Int) ?? Int((row["accuracy_m"] as? Double ?? 1000).rounded())
            // The fix rides the next send as client_location (not in the text).
            PendingClientLocation.set(AgentChatClient.ClientLocation(lat: lat, lon: lon, accuracyM: accuracy, precise: precise))
            shared = true
            onSend?("Here's my location.")
        } catch {
            shareError = (error as? AppPhoneRPCError)?.message ?? error.localizedDescription
        }
    }
}
