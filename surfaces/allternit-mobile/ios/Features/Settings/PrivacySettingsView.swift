import SwiftUI

/// Settings → Account → Privacy: the model-training preference, bulk
/// archive / delete of chats, and data export. (Moved out of the old flat
/// Settings list; same backend calls.)
struct PrivacySettingsView: View {
    @ObservedObject private var settings = SettingsStore.shared
    @State private var safariURL: IdentifiableURL? = nil
    @State private var isArchiveConfirmPresented = false
    @State private var isDeleteConfirmPresented = false
    @State private var isBulkOperationRunning = false
    @State private var bulkAlert: BulkAlert? = nil

    private struct BulkAlert: Identifiable {
        let title: String
        let message: String
        var id: String { title + message }
    }

    /// `PATCH /api/v1/agent-sessions/:id` body (`{ active: false }` archives).
    private struct UpdateSessionBody: Encodable {
        let active: Bool
    }

    var body: some View {
        List {
            Section {
                Toggle(isOn: $settings.improveModel) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Improve the model for everyone")
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        Text("Allow your chats to be used to improve Allternit's models.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                .tint(Color("AccentPrimary"))
            } footer: {
                Text("Saved on this device.")
            }

            Section {
                Button(action: { isArchiveConfirmPresented = true }) {
                    SettingsRowLabel(title: "Archive all chats", systemImage: "archivebox")
                }
                .disabled(isBulkOperationRunning)

                Button(action: { isDeleteConfirmPresented = true }) {
                    SettingsRowLabel(title: "Delete all chats", systemImage: "trash", destructive: true)
                }
                .disabled(isBulkOperationRunning)

                Button(action: {
                    safariURL = IdentifiableURL(url: URL(string: "https://ai.allternit.com/settings/privacy")!)
                }) {
                    SettingsRowLabel(title: "Export data", systemImage: "square.and.arrow.up")
                }

                Button(action: {
                    safariURL = IdentifiableURL(url: URL(string: "https://allternit.com/privacy")!)
                }) {
                    SettingsRowLabel(title: "Privacy policy", systemImage: "hand.raised")
                }
            } header: {
                Text("Your data")
            } footer: {
                if isBulkOperationRunning {
                    HStack(spacing: 8) {
                        ProgressView()
                        Text("Working…")
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Privacy")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(item: $safariURL) { item in SafariView(url: item.url) }
        .confirmationDialog("Archive all chats?", isPresented: $isArchiveConfirmPresented, titleVisibility: .visible) {
            Button("Archive All Chats") { Task { await archiveAllChats() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Archived chats are hidden from your history but kept on the server.")
        }
        .confirmationDialog("Delete all chats?", isPresented: $isDeleteConfirmPresented, titleVisibility: .visible) {
            Button("Delete All Chats", role: .destructive) { Task { await deleteAllChats() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This permanently deletes every conversation. This can't be undone.")
        }
        .alert(item: $bulkAlert) { alert in
            Alert(title: Text(alert.title), message: Text(alert.message), dismissButton: .default(Text("OK")))
        }
    }

    // MARK: - Bulk chat operations

    @MainActor
    private func archiveAllChats() async {
        await runBulkOperation(verb: "archive") { session in
            guard session.active else { return false }
            try await APIClient.shared.patch(
                path: "agent-sessions/\(Self.escape(session.id))",
                body: UpdateSessionBody(active: false)
            )
            return true
        }
    }

    @MainActor
    private func deleteAllChats() async {
        await runBulkOperation(verb: "delete") { session in
            try await APIClient.shared.delete(path: "agent-sessions/\(Self.escape(session.id))")
            return true
        }
    }

    @MainActor
    private func runBulkOperation(verb: String, operation: (AgentSession) async throws -> Bool) async {
        isBulkOperationRunning = true
        defer { isBulkOperationRunning = false }

        let sessions: [AgentSession]
        do {
            let envelope: AgentSessionListResponse = try await APIClient.shared.get(path: "agent-sessions")
            sessions = envelope.sessions
        } catch {
            bulkAlert = BulkAlert(
                title: "Couldn't \(verb) chats",
                message: "Listing conversations failed: \(FriendlyErrorMessage.from(error.localizedDescription))"
            )
            return
        }

        guard !sessions.isEmpty else {
            bulkAlert = BulkAlert(title: "Nothing to \(verb)", message: "There are no chats to \(verb).")
            return
        }

        var succeeded = 0
        var firstError: String? = nil
        for session in sessions {
            do {
                if try await operation(session) { succeeded += 1 }
            } catch {
                firstError = firstError ?? error.localizedDescription
            }
        }

        if let firstError {
            bulkAlert = BulkAlert(
                title: "Couldn't \(verb) all chats",
                message: "\(succeeded) of \(sessions.count) succeeded. First error: \(firstError)"
            )
        } else {
            bulkAlert = BulkAlert(
                title: "Done",
                message: "\(succeeded) chat\(succeeded == 1 ? "" : "s") \(verb == "archive" ? "archived" : "deleted")."
            )
        }

        if succeeded > 0 {
            NotificationCenter.default.post(name: .historyMutated, object: nil)
        }
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}

/// Icon + title row label shared by the Settings pages.
struct SettingsRowLabel: View {
    let title: String
    let systemImage: String
    var destructive: Bool = false
    var value: String? = nil
    var chevron: Bool = false

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: systemImage)
                .font(.subheadline)
                .frame(width: 22)
                .foregroundColor(destructive ? Theme.statusError : Color("TextSecondary"))
            Text(title)
                .font(.subheadline)
                .foregroundColor(destructive ? Theme.statusError : Color("TextPrimary"))
            Spacer(minLength: 8)
            if let value {
                Text(value)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            if chevron {
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .contentShape(Rectangle())
    }
}
