import SwiftUI

extension TeamAppRef: Identifiable {
    var id: String { "\(kind):\(ref)" }
}

/// "Who can use this": for one connection or app, the people and bots that
/// can use it, each with a level chip (Browser / App / Full). Same rows and
/// wording as ai `WhoCanUse` (#1314). Opened from an app row on a bot's team
/// map page and from a connected connector.
struct TeamWhoCanUseSheet: View {
    let app: TeamAppRef
    @ObservedObject var store: TeamMapStore

    @State private var access: TeamAppAccess?
    @State private var failure: TeamMapError?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Who can use \(app.displayName)")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("Done") { dismiss() }
                    }
                }
        }
        .task {
            if store.map == nil { await store.load() }
            await load()
        }
    }

    @ViewBuilder
    private var content: some View {
        if let failure {
            VStack(spacing: 12) {
                if failure == .noRuntime {
                    Text("Connect a computer to see who can use this.")
                } else {
                    Text(failure.errorDescription ?? "That didn't work. Try again.")
                    Button("Try again") { Task { await load() } }
                        .frame(minHeight: 44)
                }
            }
            .font(.subheadline)
            .foregroundStyle(Color("TextSecondary"))
            .multilineTextAlignment(.center)
            .padding(24)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let access {
            list(access)
        } else {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func list(_ access: TeamAppAccess) -> some View {
        List {
            Section("People") {
                if access.people.isEmpty {
                    Text("No one yet.").font(.footnote).foregroundStyle(Color("TextSecondary"))
                }
                ForEach(access.people) { person in
                    HStack(spacing: 10) {
                        Image(systemName: "person.crop.circle").foregroundStyle(Color("TextSecondary")).accessibilityHidden(true)
                        Text(name(person.user)).foregroundStyle(Color("TextPrimary"))
                        Spacer()
                        chip(person.level)
                    }
                    .frame(minHeight: 44)
                    .accessibilityElement(children: .combine)
                }
            }
            Section("Bots") {
                if access.bots.isEmpty {
                    Text("No bots yet.").font(.footnote).foregroundStyle(Color("TextSecondary"))
                }
                ForEach(access.bots) { bot in
                    HStack(spacing: 10) {
                        Image(systemName: "cpu").foregroundStyle(Color("TextSecondary")).accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(name(bot.bot)).foregroundStyle(Color("TextPrimary"))
                            Text(botDetail(bot))
                                .font(.caption)
                                .foregroundStyle(Color("TextSecondary"))
                        }
                        Spacer()
                        chip(bot.level)
                    }
                    .frame(minHeight: 44)
                    .accessibilityElement(children: .combine)
                }
            } footer: {
                if app.kind == "vendor" || appCeiling == "app" {
                    Text("Vendor app: levels stop at App.")
                } else {
                    Text("Change a bot's level from its page on the team map.")
                }
            }
        }
        .listStyle(.insetGrouped)
        .refreshable { await load() }
    }

    private func chip(_ level: String) -> some View {
        Text(TeamLevel.label(level))
            .font(.footnote.weight(.medium))
            .foregroundStyle(Color("TextPrimary"))
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .background(Capsule().fill(Color("FillNeutral")))
    }

    private func name(_ id: String) -> String {
        store.map?.name(id) ?? String(id.dropFirst(2))
    }

    private func grant(for bot: String) -> TeamGrant? {
        store.map?.grants.first { $0.bot == bot && $0.app.kind == app.kind && $0.app.ref == app.ref }
    }

    private func botDetail(_ bot: TeamAppAccess.Bot) -> String {
        let owner = bot.owner ?? store.map?.owner(of: bot.bot)
        var text = owner.map { "\(name($0))'s" } ?? "No owner"
        if let ceiling = grant(for: bot.bot)?.ceiling, ceiling != "full" {
            text += " · stops at \(TeamLevel.label(ceiling))"
        }
        return text
    }

    private var appCeiling: String? {
        let ceilings = (store.map?.grants ?? [])
            .filter { $0.app.kind == app.kind && $0.app.ref == app.ref }
            .map(\.ceiling)
        guard let first = ceilings.first, ceilings.allSatisfy({ $0 == first }) else { return nil }
        return first
    }

    private func load() async {
        failure = nil
        do {
            access = try await store.appAccess(kind: app.kind, ref: app.ref)
        } catch is CancellationError {
            return
        } catch let error as TeamMapError {
            failure = error
        } catch {
            failure = .storeUnavailable
        }
    }
}

/// "Who can use this" for a screen that has no team map store of its own
/// (the Connectors list): owns one and shows the sheet.
struct TeamWhoCanUseHost: View {
    let app: TeamAppRef
    @StateObject private var store = TeamMapStore()

    var body: some View {
        // The Connectors list knows a connector by its catalog id ("stripe");
        // the team map keys apps by the MCP connector id ("mcp_stripe"). Like
        // web (#1314 whoRefs), use the app as the map knows it.
        TeamWhoCanUseSheet(app: Self.resolve(app, in: store.map), store: store)
            .id(Self.resolve(app, in: store.map).id)
    }

    /// The map's own ref for `app`: same ref, `mcp_<ref>`, or the same name
    /// (case-insensitive) among the map's grants of that kind. Unchanged when
    /// the map is not loaded or nothing matches (the sheet then shows nobody).
    static func resolve(_ app: TeamAppRef, in map: TeamMap?) -> TeamAppRef {
        guard let map else { return app }
        let known = map.grants.map(\.app).filter { $0.kind == app.kind }
        if let hit = known.first(where: { $0.ref == app.ref || $0.ref == "mcp_\(app.ref)" }) { return hit }
        if let name = app.name?.lowercased(),
           let hit = known.first(where: { $0.name?.lowercased() == name }) { return hit }
        return app
    }
}
