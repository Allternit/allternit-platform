import SwiftUI
import UIKit

// "Continue CLI session" (allternit-ai src/components/native-sessions/
// NativeSessionPicker.tsx + lib/agents/native-sessions-api.ts): a read-only
// catalog of Claude Code, Codex, Grok… sessions stored on your computer.
// Picking one snapshots it into Allternit as a new session (the original
// file isn't changed) and opens it.
//
//   GET  native-sessions/harnesses          → { harnesses: [...] }
//   GET  native-sessions[?harness=]         → { sessions: [...] }
//   POST native-sessions/pickup             → { session: { id, title } }
// Plain APIClient paths: cloud-api relays them to the default computer.

struct NativeHarnessInfo: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let present: Bool

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        label = try c.decodeIfPresent(String.self, forKey: .label) ?? id
        present = try c.decodeIfPresent(Bool.self, forKey: .present) ?? false
    }

    enum CodingKeys: String, CodingKey { case id, label, present }
}

struct NativeCatalogSession: Decodable, Identifiable, Hashable, Sendable {
    let harness: String
    let sessionId: String
    let path: String
    let cwd: String?
    let title: String?
    /// Epoch milliseconds.
    let updatedAt: Double

    var id: String { "\(harness):\(sessionId)" }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        harness = try c.decode(String.self, forKey: .harness)
        sessionId = try c.decode(String.self, forKey: .sessionId)
        path = try c.decodeIfPresent(String.self, forKey: .path) ?? ""
        cwd = try c.decodeIfPresent(String.self, forKey: .cwd)
        title = try c.decodeIfPresent(String.self, forKey: .title)
        updatedAt = try c.decodeIfPresent(Double.self, forKey: .updatedAt) ?? 0
    }

    enum CodingKeys: String, CodingKey { case harness, sessionId, path, cwd, title, updatedAt }
}

private struct NativeHarnessList: Decodable { let harnesses: [NativeHarnessInfo]? }
private struct NativeSessionList: Decodable { let sessions: [NativeCatalogSession]? }

private struct PickupBody: Encodable {
    let harness: String
    let sessionId: String
    let surface: String
    let cwd: String?
}

private struct PickupResult: Decodable {
    struct Session: Decodable { let id: String }
    let session: Session
}

enum NativeSessionsClient {
    static func harnesses() async throws -> [NativeHarnessInfo] {
        let list: NativeHarnessList = try await APIClient.shared.get(path: "native-sessions/harnesses")
        return list.harnesses ?? []
    }

    static func sessions(harness: String?) async throws -> [NativeCatalogSession] {
        var path = "native-sessions"
        if let harness, let escaped = harness.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) {
            path += "?harness=\(escaped)"
        }
        let list: NativeSessionList = try await APIClient.shared.get(path: path)
        return list.sessions ?? []
    }

    /// Snapshots the native session into Allternit; returns the new session id.
    static func pickup(_ item: NativeCatalogSession, surface: String) async throws -> String {
        let result: PickupResult = try await APIClient.shared.post(
            path: "native-sessions/pickup",
            body: PickupBody(harness: item.harness, sessionId: item.sessionId, surface: surface, cwd: item.cwd)
        )
        return result.session.id
    }

    /// The web's errors for a backend without the catalog ("update Desktop").
    static func friendly(_ error: Error) -> String {
        if case APIError.httpError(let status, _) = error, status == 404 {
            return "Your computer's Allternit doesn't have CLI sessions yet. Update Allternit Desktop, then try again."
        }
        if case APIError.decoding = error {
            return "Your computer's Allternit doesn't have CLI sessions yet. Update Allternit Desktop, then try again."
        }
        return FriendlyErrorMessage.from(error.localizedDescription)
    }
}

struct NativeSessionPickerSheet: View {
    /// "chat" or "code" — where the picked-up session opens.
    let surface: String
    var onPicked: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var auth = AuthManager.shared
    @State private var harnesses: [NativeHarnessInfo] = []
    @State private var sessions: [NativeCatalogSession] = []
    @State private var harness = "all"
    @State private var query = ""
    @State private var isLoading = true
    @State private var loadError: String? = nil
    @State private var pickError: String? = nil
    @State private var picking: String? = nil

    private var filtered: [NativeCatalogSession] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        let list = q.isEmpty ? sessions : sessions.filter { item in
            [item.harness, item.sessionId, item.title ?? "", item.cwd ?? "", item.path]
                .joined(separator: " ").lowercased().contains(q)
        }
        return Array(list.prefix(200))
    }

    var body: some View {
        NavigationStack {
            List {
                if !auth.isSignedIn && !auth.isSkippingAuth {
                    FriendlyStateView(style: .empty, icon: "person.crop.circle", title: "Sign in to continue a CLI session",
                                      message: "CLI sessions are read from the computer paired to your account.",
                                      actionTitle: "Sign in", action: {
                                          dismiss()
                                          auth.isPresentingAuth = true
                                      })
                        .listRowBackground(Color.clear)
                } else {
                    Section {
                        Text("Sessions from Claude Code, Codex, Grok and other CLIs on your computer. Picking one copies it into Allternit; the original isn't changed.")
                            .font(.footnote)
                            .foregroundColor(Color("TextSecondary"))
                        if harnesses.contains(where: \.present) {
                            Picker("CLI", selection: $harness) {
                                Text("All CLIs").tag("all")
                                ForEach(harnesses.filter(\.present)) { Text($0.label).tag($0.id) }
                            }
                        }
                    }
                    if let pickError {
                        Text(pickError)
                            .font(.footnote)
                            .foregroundColor(Theme.statusError)
                    }
                    results
                }
            }
            .searchable(text: $query, prompt: "Search id, title, folder…")
            .navigationTitle("Continue a CLI session")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            }
            .refreshable { await load() }
            .task(id: harness) { await load() }
        }
    }

    @ViewBuilder
    private var results: some View {
        if isLoading && sessions.isEmpty {
            HStack(spacing: 10) {
                ProgressView()
                Text("Looking on your computer…").foregroundColor(Color("TextSecondary"))
            }
        } else if let loadError, sessions.isEmpty {
            FriendlyStateView(style: .offline, icon: "desktopcomputer.trianglebadge.exclamationmark",
                              title: "Couldn't read CLI sessions", message: loadError,
                              actionTitle: "Retry", action: { Task { await load() } })
                .listRowBackground(Color.clear)
        } else if filtered.isEmpty {
            Text(query.isEmpty ? "No CLI sessions found on your computer." : "Nothing matches.")
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
        } else {
            Section {
                ForEach(filtered) { item in
                    Button { pick(item) } label: { row(item) }
                        .buttonStyle(.plain)
                        .disabled(picking != nil)
                }
            }
        }
    }

    private func row(_ item: NativeCatalogSession) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Text(item.harness.uppercased())
                .font(.system(size: 10, weight: .bold))
                .foregroundColor(Color("TextSecondary"))
                .padding(.horizontal, 6)
                .padding(.vertical, 3)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color("BgSecondary")))
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title?.isEmpty == false ? item.title! : item.sessionId)
                    .font(.subheadline.weight(.medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text([item.cwd ?? item.path, Self.when(item.updatedAt)].filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            Spacer(minLength: 4)
            if picking == item.id {
                ProgressView()
            } else {
                Image(systemName: "arrow.right")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .contentShape(Rectangle())
    }

    private static func when(_ ms: Double) -> String {
        guard ms > 0 else { return "" }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .short
        return formatter.localizedString(for: Date(timeIntervalSince1970: ms / 1000), relativeTo: Date())
    }

    private func load() async {
        isLoading = true
        loadError = nil
        do {
            async let h = NativeSessionsClient.harnesses()
            async let s = NativeSessionsClient.sessions(harness: harness == "all" ? nil : harness)
            let (hs, ss) = try await (h, s)
            harnesses = hs
            sessions = ss.sorted { $0.updatedAt > $1.updatedAt }
        } catch is CancellationError {
        } catch {
            loadError = NativeSessionsClient.friendly(error)
        }
        isLoading = false
    }

    private func pick(_ item: NativeCatalogSession) {
        picking = item.id
        pickError = nil
        Task {
            defer { picking = nil }
            do {
                let sessionId = try await NativeSessionsClient.pickup(item, surface: surface)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                NotificationCenter.default.post(name: .historyMutated, object: nil)
                dismiss()
                onPicked(sessionId)
            } catch {
                pickError = "Couldn't continue that session: \(NativeSessionsClient.friendly(error))"
            }
        }
    }
}
