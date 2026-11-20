import SwiftUI
import UIKit

/// Money for a cap or spend line: whole dollars when there are no cents.
enum GrantMoney {
    static func text(_ cents: Int) -> String {
        cents % 100 == 0 ? "$\(cents / 100)" : String(format: "$%.2f", Double(cents) / 100)
    }

    /// "Spent $1.23 of $10 this month".
    static func spentLine(_ grant: OAuthGrant) -> String {
        "Spent \(text(grant.spentCents)) of \(text(grant.monthlyCapCents)) this month"
    }
}

@MainActor
final class ConnectedAppsModel: ObservableObject {
    enum Phase: Equatable { case loading, loaded, failed(String) }

    @Published private(set) var phase: Phase = .loading
    @Published private(set) var grants: [OAuthGrant] = []
    @Published private(set) var resetsAt: String?

    private let client: OAuthGrantsClient
    init(client: OAuthGrantsClient = .shared) { self.client = client }

    func load() async {
        if grants.isEmpty { phase = .loading }
        do {
            let list = try await client.list()
            grants = list.data
            resetsAt = list.resetsAt
            phase = .loaded
        } catch is CancellationError {
            return
        } catch {
            phase = .failed(error.localizedDescription)
        }
    }

    func setCap(_ grant: OAuthGrant, cents: Int) async throws {
        let updated = try await client.setCap(id: grant.id, cents: cents)
        if let i = grants.firstIndex(where: { $0.id == updated.id }) { grants[i] = updated }
    }

    func revoke(_ grant: OAuthGrant) async throws {
        try await client.revoke(id: grant.id)
        grants.removeAll { $0.id == grant.id }
    }
}

/// Settings › Privacy › Connected apps: outside apps that use your Allternit
/// account and credits, each with a monthly cap you can change or end.
struct ConnectedAppsView: View {
    @StateObject private var model = ConnectedAppsModel()
    @State private var selected: OAuthGrant?

    var body: some View {
        PhoneSettingsPage(title: "Connected apps") {
            SettingsDescription(text: "Apps you approved to use your Allternit credits. Each one stops at its monthly cap, and you can end access at any time.")
            switch model.phase {
            case .loading:
                HStack { Spacer(); ProgressView(); Spacer() }
                    .padding(.vertical, 24)
                    .accessibilityLabel("Loading connected apps")
            case .failed(let message):
                SettingsErrorBanner(message: "Couldn't load your connected apps. \(message)")
                Button("Try again") { Task { await model.load() } }.buttonStyle(ConnQuietButtonStyle())
            case .loaded:
                if model.grants.isEmpty {
                    Text("No apps have access to your account.")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.top, 8)
                        .accessibilityIdentifier("connected-apps-empty")
                } else {
                    PhoneSettingsCard {
                        ForEach(Array(model.grants.enumerated()), id: \.element.id) { i, grant in
                            if i > 0 { PhoneCardDivider() }
                            Button { selected = grant } label: { ConnectedAppRow(grant: grant) }
                                .buttonStyle(PhoneRowButtonStyle())
                                .accessibilityIdentifier("connected-app-\(grant.id)")
                        }
                    }
                }
            }
        }
        .refreshable { await model.load() }
        .task { await model.load() }
        .sheet(item: $selected) { grant in
            ConnectedAppDetailSheet(grant: grant, model: model)
        }
        .accessibilityIdentifier("connected-apps-page")
    }
}

private struct ConnectedAppLogo: View {
    let grant: OAuthGrant
    var size: CGFloat = 36

    var body: some View {
        let initial = Text(String(grant.appName.prefix(1)).uppercased())
            .font(.system(size: size * 0.45, weight: .semibold))
            .foregroundColor(Color("TextPrimary"))
            .frame(width: size, height: size)
            .background(Color("TextPrimary").opacity(0.07))
        Group {
            if let raw = grant.appLogoURL, let url = URL(string: raw) {
                AsyncImage(url: url) { phase in
                    if let image = phase.image {
                        image.resizable().scaledToFit().padding(size * 0.15).frame(width: size, height: size).background(Color.white)
                    } else {
                        initial
                    }
                }
            } else {
                initial
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: max(6, (size / 4).rounded()), style: .continuous))
        .accessibilityHidden(true)
    }
}

private struct TestingTag: View {
    var body: some View {
        Text("Testing app")
            .font(.system(size: 11, weight: .medium))
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .background(Capsule().fill(Color("TextPrimary").opacity(0.07)))
    }
}

private struct ConnectedAppRow: View {
    let grant: OAuthGrant

    var body: some View {
        HStack(spacing: 12) {
            ConnectedAppLogo(grant: grant)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(grant.appName)
                        .font(.system(size: 16, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if grant.isTesting { TestingTag() }
                }
                Text(GrantMoney.spentLine(grant))
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            PhoneRowChevron()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
        .contentShape(Rectangle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(grant.appName)\(grant.isTesting ? ", testing app" : ""). \(GrantMoney.spentLine(grant))")
        .accessibilityAddTraits(.isButton)
    }
}

/// The app's detail: its monthly cap and Revoke.
private struct ConnectedAppDetailSheet: View {
    let grant: OAuthGrant
    @ObservedObject var model: ConnectedAppsModel

    @Environment(\.dismiss) private var dismiss
    @State private var capDollars: Int
    @State private var saving = false
    @State private var revoking = false
    @State private var confirmRevoke = false
    @State private var error: String?

    init(grant: OAuthGrant, model: ConnectedAppsModel) {
        self.grant = grant
        self.model = model
        _capDollars = State(initialValue: max(1, grant.monthlyCapCents / 100))
    }

    /// The live row, so a saved cap shows without reopening.
    private var current: OAuthGrant { model.grants.first { $0.id == grant.id } ?? grant }
    /// While testing the server limits the cap; once live only the person does.
    private var maxDollars: Int { current.maxCapCents.map { max(1, $0 / 100) } ?? 1000 }
    private var changed: Bool { capDollars * 100 != current.monthlyCapCents }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    HStack(spacing: 12) {
                        ConnectedAppLogo(grant: current, size: 44)
                        VStack(alignment: .leading, spacing: 3) {
                            HStack(spacing: 6) {
                                Text(current.appName).font(.system(size: 18, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                                if current.isTesting { TestingTag() }
                            }
                            Text(GrantMoney.spentLine(current)).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                        }
                    }
                    if current.isTesting {
                        Text("This app is still being tested by its developer. Its cap can't go above \(GrantMoney.text(current.maxCapCents ?? 0)) until it is approved.")
                            .font(.system(size: 13)).foregroundColor(Color("TextSecondary")).fixedSize(horizontal: false, vertical: true)
                    }
                    VStack(alignment: .leading, spacing: 10) {
                        Stepper(value: $capDollars, in: 1...maxDollars) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("Monthly cap").font(.system(size: 15, weight: .medium)).foregroundColor(Color("TextPrimary"))
                                Text("$\(capDollars) a month").font(.system(size: 13).monospacedDigit()).foregroundColor(Color("TextSecondary"))
                            }
                        }
                        .accessibilityValue("\(capDollars) dollars a month")
                        .onChange(of: capDollars) { _, _ in UISelectionFeedbackGenerator().selectionChanged() }
                        Button { Task { await save() } } label: {
                            HStack(spacing: 6) {
                                if saving { ProgressView().controlSize(.small).tint(Color("BgPrimary")) }
                                Text("Save cap")
                            }
                        }
                        .buttonStyle(ConnPrimaryButtonStyle())
                        .disabled(!changed || saving || revoking)
                    }
                    .padding(12)
                    .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("FillNeutral")))
                    if let error { SettingsErrorBanner(message: error) }
                    Button(role: .destructive) { confirmRevoke = true } label: {
                        HStack(spacing: 6) {
                            if revoking { ProgressView().controlSize(.small) }
                            Text("Revoke access")
                        }
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Theme.statusError)
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.statusError.opacity(0.1)))
                    }
                    .disabled(saving || revoking)
                    .accessibilityIdentifier("connected-app-revoke")
                }
                .padding(EdgeInsets(top: 8, leading: 16, bottom: 24, trailing: 16))
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Connected app")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } } }
            .confirmationDialog("Revoke \(current.appName)?", isPresented: $confirmRevoke, titleVisibility: .visible) {
                Button("Revoke access", role: .destructive) { Task { await revoke() } }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("\(current.appName) stops working with your account right away. You can approve it again later.")
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    private func save() async {
        error = nil
        saving = true
        defer { saving = false }
        do {
            try await model.setCap(current, cents: capDollars * 100)
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        } catch {
            self.error = error.localizedDescription
            UINotificationFeedbackGenerator().notificationOccurred(.error)
        }
    }

    private func revoke() async {
        error = nil
        revoking = true
        defer { revoking = false }
        do {
            try await model.revoke(current)
            UINotificationFeedbackGenerator().notificationOccurred(.warning)
            dismiss()
        } catch {
            self.error = "Couldn't revoke \(current.appName). \(error.localizedDescription)"
        }
    }
}
