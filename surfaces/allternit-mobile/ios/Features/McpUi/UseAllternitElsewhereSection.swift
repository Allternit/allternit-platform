import SwiftUI

/// "Use Allternit in other apps": the MCP URL with Copy, per-client steps, and the
/// apps already connected (from the approvals list) with Disconnect.
struct UseAllternitElsewhereSection: View {
    static let url = "https://mcp.allternit.com"
    private static let steps: [(String, String)] = [
        ("Claude", "Settings > Connectors > Add custom connector, paste the URL, sign in."),
        ("ChatGPT", "Settings > Connectors > Create, paste the URL, choose OAuth."),
        ("Cursor", "Settings > MCP > Add server, paste the URL."),
        ("Gizzi", "Run: gizzi mcp add allternit \(url)"),
    ]
    @State private var client = "Claude"
    @State private var copied = false
    @State private var apps: [McpConnectedApp] = []
    @State private var loaded = false
    @State private var error: String?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Section {
            Text("Use Allternit in other apps").font(.headline)
            HStack(spacing: 8) {
                Image(systemName: "app.connected.to.app.below.fill")
                    .symbolEffect(.bounce, options: .nonRepeating, value: reduceMotion ? false : copied)
                Text(Self.url).font(.system(size: 13, design: .monospaced)).lineLimit(1).minimumScaleFactor(0.8)
                Spacer(minLength: 4)
                Button {
                    UIPasteboard.general.string = Self.url
                    Haptics.light()
                    copied = true
                } label: {
                    Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                        .font(.footnote).contentTransition(.symbolEffect(.replace))
                        .frame(minHeight: 44)
                }
                .buttonStyle(.plain)
            }
            HStack(spacing: 6) {
                ForEach(Self.steps, id: \.0) { name, _ in
                    Button(name) { client = name }
                        .font(.footnote)
                        .padding(.horizontal, 12).frame(minHeight: 44)
                        .background(client == name ? Color("FillNeutral") : .clear, in: Capsule())
                        .foregroundColor(Color("TextPrimary"))
                        .buttonStyle(.plain)
                }
            }
            Text(Self.steps.first { $0.0 == client }?.1 ?? "").font(.footnote)
            Text("Connected apps").font(.subheadline.weight(.semibold)).padding(.top, 4)
            if let error { Text(error).font(.footnote).foregroundColor(Theme.statusError) }
            if loaded && apps.isEmpty { Text("No apps connected.").font(.footnote).foregroundColor(Color("TextSecondary")) }
            ForEach(apps) { app in
                HStack(spacing: 8) {
                    Image(systemName: "app.connected.to.app.below.fill")
                    VStack(alignment: .leading, spacing: 2) {
                        Text(app.clientName).font(.subheadline)
                        Text(subtitle(app)).font(.caption).foregroundColor(Color("TextSecondary"))
                        if !app.tools.isEmpty {
                            Text("Can use: \(app.tools.joined(separator: ", "))").font(.caption)
                                .foregroundColor(Color("TextSecondary")).lineLimit(2)
                        }
                    }
                    Spacer(minLength: 4)
                    Button { Task { await disconnect(app) } } label: {
                        Label("Disconnect", systemImage: "link.badge.minus").font(.footnote).frame(minHeight: 44)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .task { await load() }
    }

    private func subtitle(_ app: McpConnectedApp) -> String {
        guard let last = app.lastUsedAt,
              let date = ISO8601DateFormatter().date(from: last) ?? ISO8601DateFormatter.fractional.date(from: last)
        else { return "Not used yet" }
        return "Last used " + RelativeDateTimeFormatter().localizedString(for: date, relativeTo: Date())
    }

    @MainActor private func load() async {
        do { apps = try await McpServersClient.shared.connectedApps(); error = nil } catch { self.error = "Couldn't load connected apps." }
        loaded = true
    }

    @MainActor private func disconnect(_ app: McpConnectedApp) async {
        do {
            try await McpServersClient.shared.disconnectApp(app)
            apps.removeAll { $0.id == app.id }
        } catch { self.error = "Couldn't disconnect \(app.clientName). Try again." }
    }
}

private extension ISO8601DateFormatter {
    nonisolated(unsafe) static let fractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
}
