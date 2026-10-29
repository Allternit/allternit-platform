import SwiftUI
import UIKit

/// The drawer in Bots mode: the desktop Bots rail, touch-sized (PhoneDrawer
/// BotRail → ShellRail mode="bot" → BotModeRail). Top to bottom: Home and
/// the rail's ⋯, the main bot tile, the promoted row, Projects, the Bots
/// list, and Group chats pinned at the bottom above the drawer's footer.
struct BotsDrawerRail: View {
    /// Opens a Bots screen and closes the drawer (nil route = the Bots home).
    let onOpen: (BotRoute?) -> Void
    /// A bot opens on its main chat.
    let onOpenBot: (String) -> Void
    /// The Projects heading: the Projects list.
    let onOpenProjects: () -> Void

    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var hub = AgentHubStore.shared
    @AppStorage("allternit-rail-bot-projects") private var projectsExpanded = true
    @AppStorage("allternit-rail-bot-bots") private var botsExpanded = true
    @AppStorage("allternit-rail-bot-groups") private var groupsExpanded = true

    @State private var isNewBotPresented = false
    @State private var isVendorPresented = false
    @State private var isTemplatesPresented = false
    @State private var isMainSetupPresented = false
    @State private var isNewProjectPresented = false
    @State private var newProjectName = ""
    @State private var railError: String?

    private static let maxPromoted = 3

    private var bots: [AgentRecord] { store.bots }
    private var mainBot: AgentRecord? { bots.first(where: \.isMainBot) }
    private var promoted: [AgentRecord] {
        store.pinnedBotIds
            .filter { $0 != mainBot?.id }
            .compactMap { id in bots.first { $0.id == id } }
            .prefix(Self.maxPromoted)
            .map { $0 }
    }
    /// Every bot that is neither main nor promoted, latest activity first.
    private var listBots: [AgentRecord] {
        let promotedIds = Set(promoted.map(\.id))
        return bots
            .filter { $0.id != mainBot?.id && !promotedIds.contains($0.id) }
            .sorted { a, b in
                let aa = latestThread(a.id)?.lastActivityAt ?? "", ba = latestThread(b.id)?.lastActivityAt ?? ""
                if aa != ba { return aa > ba }
                return a.botDisplayName.localizedCaseInsensitiveCompare(b.botDisplayName) == .orderedAscending
            }
    }
    private var projects: [BotProjectSummary] {
        store.projects
            .filter { !$0.archived }
            .sorted { a, b in
                if a.favorite != b.favorite { return a.favorite }
                return (a.lastActivityAt ?? "") > (b.lastActivityAt ?? "")
            }
    }
    private var groups: [BotGroupChat] {
        store.groups.sorted { $0.updatedAt > $1.updatedAt }
    }

    var body: some View {
        VStack(spacing: 0) {
            topRow
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    mainBotTile
                    promotedRow
                    projectsSection
                    botsSection
                }
                .padding(.horizontal, 6)
                .padding(.bottom, 12)
            }
            groupsSection
                // Clears the floating footer (avatar, Phones, New thread).
                .padding(.bottom, 84)
        }
        .task { await store.refreshAll() }
        .sheet(isPresented: $isNewBotPresented) {
            NewBotSheet { _ in Task { await store.refreshAll() } }
        }
        .sheet(isPresented: $isVendorPresented) {
            NavigationStack {
                VendorConnectView { _, _ in
                    isVendorPresented = false
                    hub.fetchAgentsIfNeeded(force: true)
                }
                .navigationTitle("Add a vendor bot")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Close") { isVendorPresented = false } }
                }
            }
        }
        .sheet(isPresented: $isTemplatesPresented) {
            AgentTemplateSheet { template in
                Task {
                    do {
                        _ = try await hub.createFromTemplate(templateId: template.id)
                        hub.fetchAgentsIfNeeded(force: true)
                    } catch {
                        railError = FriendlyErrorMessage.from(error.localizedDescription)
                    }
                }
            }
        }
        .sheet(isPresented: $isMainSetupPresented) {
            MainBotSetupSheet(bots: bots) { botId in
                hub.fetchAgentsIfNeeded(force: true)
                onOpenBot(botId)
            }
            .presentationDetents([.medium, .large])
        }
        .alert("New project", isPresented: $isNewProjectPresented) {
            TextField("Name", text: $newProjectName)
            Button("Cancel", role: .cancel) { newProjectName = "" }
            Button("Create") { createProject() }
        } message: {
            Text("Name it and tell the bots what it's for in its chat.")
        }
        .alert("Something went wrong", isPresented: Binding(
            get: { railError != nil }, set: { if !$0 { railError = nil } }
        )) {
            Button("OK", role: .cancel) { railError = nil }
        } message: { Text(railError ?? "") }
    }

    // MARK: - Home and ⋯

    private var topRow: some View {
        HStack(spacing: 4) {
            Button {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                onOpen(nil)
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: "house").font(.system(size: 14))
                    Text("Home").font(.system(size: 14, weight: .medium))
                }
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 12)
                .frame(height: 36)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Spacer(minLength: 0)
            // One ⋯: Create bot · Add a vendor bot · Marketplace · Replay intro (BotRailOverflow).
            Menu {
                Button { isNewBotPresented = true } label: { Label("Create bot", systemImage: "plus") }
                Button { isVendorPresented = true } label: { Label("Add a vendor bot…", systemImage: "powerplug") }
                Button { isTemplatesPresented = true } label: { Label("Marketplace", systemImage: "storefront") }
                Button {
                    onOpen(nil)
                    BotsIntroStore.shared.replay()
                } label: { Label("Replay intro", systemImage: "arrow.counterclockwise") }
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 17, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("More")
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 4)
    }

    // MARK: - Main bot

    @ViewBuilder
    private var mainBotTile: some View {
        if let mainBot {
            Button { onOpenBot(mainBot.id) } label: {
                VStack(spacing: 6) {
                    LiveBotStatusAvatar(bot: mainBot, size: 72)
                        .overlay(alignment: .bottomTrailing) {
                            Image(systemName: "crown.fill")
                                .font(.system(size: 11))
                                .foregroundColor(.white)
                                .frame(width: 24, height: 24)
                                .background(Circle().fill(Theme.statusInfo))
                                .overlay(Circle().stroke(Color("BgPrimary"), lineWidth: 2))
                                .offset(x: 2, y: 2)
                        }
                    Text(mainBot.botDisplayName)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                }
                .padding(.horizontal, 8)
                .padding(.top, 10)
                .padding(.bottom, 8)
                .frame(width: 116)
                .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            }
            .buttonStyle(.plain)
            .frame(maxWidth: .infinity)
            .padding(.top, 4)
            .padding(.bottom, 8)
            .accessibilityLabel("Open \(mainBot.botDisplayName), your main bot")
        } else {
            // Everyone gets a main bot: the empty tile opens its setup.
            Button { isMainSetupPresented = true } label: {
                VStack(spacing: 6) {
                    Image(systemName: "crown")
                        .font(.system(size: 19))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 56, height: 56)
                        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
                    Text("Set up your main bot")
                        .font(.system(size: 12.5, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .multilineTextAlignment(.center)
                }
                .padding(.horizontal, 8)
                .padding(.top, 10)
                .padding(.bottom, 8)
                .frame(width: 132)
                .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .stroke(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
                .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            }
            .buttonStyle(.plain)
            .frame(maxWidth: .infinity)
            .padding(.top, 4)
            .padding(.bottom, 8)
        }
    }

    // MARK: - Promoted

    /// Up to three promoted bots under the main bot (the roster's pins), then
    /// a Promote slot. Nothing to promote and nothing promoted: no row.
    @ViewBuilder
    private var promotedRow: some View {
        let candidates = listBots
        if !promoted.isEmpty || !candidates.isEmpty {
            HStack(alignment: .top, spacing: 4) {
                ForEach(promoted) { bot in
                    VStack(spacing: 4) {
                        Button { onOpenBot(bot.id) } label: {
                            LiveBotStatusAvatar(bot: bot, size: 40)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Open \(bot.botDisplayName)")
                        Text(bot.botDisplayName)
                            .font(.system(size: 11, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                    }
                    .frame(width: 76)
                    .overlay(alignment: .topTrailing) {
                        Menu {
                            Button { store.togglePin(bot.id) } label: { Label("Unpromote", systemImage: "pin.slash") }
                        } label: {
                            Image(systemName: "ellipsis")
                                .font(.system(size: 10, weight: .bold))
                                .foregroundColor(Color("TextTertiary"))
                                .frame(width: 20, height: 20)
                                .background(Circle().fill(Color("BgPanel")))
                                .overlay(Circle().stroke(Color("BorderSubtle")))
                                .frame(width: 32, height: 32)
                                .contentShape(Rectangle())
                        }
                        .offset(x: 6, y: -8)
                        .accessibilityLabel("More for \(bot.botDisplayName)")
                    }
                }
                if promoted.count < Self.maxPromoted {
                    Menu {
                        Section("Promoted bots sit here, up to \(Self.maxPromoted).") {
                            if candidates.isEmpty {
                                Text("No other bots to promote yet.")
                            } else {
                                ForEach(candidates) { bot in
                                    Button(bot.botDisplayName) { store.togglePin(bot.id) }
                                }
                            }
                        }
                    } label: {
                        VStack(spacing: 4) {
                            Image(systemName: "plus")
                                .font(.system(size: 13))
                                .foregroundColor(Color("TextTertiary"))
                                .frame(width: 40, height: 40)
                                .overlay(Circle().stroke(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
                            Text("Promote")
                                .font(.system(size: 11, weight: .medium))
                                .foregroundColor(Color("TextTertiary"))
                        }
                        .frame(width: 76)
                    }
                    .accessibilityLabel("Promote a bot")
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .padding(.bottom, 8)
        }
    }

    // MARK: - Sections

    private func sectionHeader(_ title: String, expanded: Binding<Bool>, onTitle: (() -> Void)? = nil,
                               addLabel: String? = nil, onAdd: (() -> Void)? = nil) -> some View {
        HStack(spacing: 0) {
            Button {
                withAnimation(.easeOut(duration: 0.16)) { expanded.wrappedValue.toggle() }
            } label: {
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundColor(Color("TextTertiary"))
                    .rotationEffect(.degrees(expanded.wrappedValue ? 90 : 0))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(expanded.wrappedValue ? "Collapse \(title.lowercased())" : "Expand \(title.lowercased())")
            Button {
                if let onTitle { onTitle() } else {
                    withAnimation(.easeOut(duration: 0.16)) { expanded.wrappedValue.toggle() }
                }
            } label: {
                Text(title.uppercased())
                    .font(.system(size: 11, weight: .semibold))
                    .tracking(0.66)
                    .foregroundColor(Color("TextTertiary"))
                    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if let onAdd {
                Button(action: onAdd) {
                    Image(systemName: "plus")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(addLabel ?? "Add")
            }
        }
    }

    private var projectsSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            sectionHeader("Projects", expanded: $projectsExpanded, onTitle: onOpenProjects,
                          addLabel: "New project") { isNewProjectPresented = true }
            if projectsExpanded {
                if projects.isEmpty {
                    emptyLine("No projects yet.")
                } else {
                    ForEach(projects) { project in
                        Button { onOpen(.project(project)) } label: {
                            HStack(spacing: 10) {
                                Image(systemName: project.favorite ? "star.fill" : "folder")
                                    .font(.system(size: 15))
                                    .foregroundColor(project.favorite ? Theme.statusWarning : Color("TextSecondary"))
                                    .frame(width: 22)
                                Text(project.title)
                                    .font(.system(size: 15))
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(1)
                                Spacer(minLength: 6)
                                if project.counts.waiting > 0 {
                                    Text("\(project.counts.waiting) waiting")
                                        .font(.system(size: 11, weight: .medium))
                                        .foregroundColor(Theme.statusWarning)
                                } else if project.counts.working > 0 {
                                    Text("\(project.counts.working) working")
                                        .font(.system(size: 11, weight: .medium))
                                        .foregroundColor(Theme.statusSuccess)
                                }
                            }
                            .padding(.horizontal, 12)
                            .frame(minHeight: 44)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .padding(.bottom, 6)
    }

    private var botsSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            sectionHeader("Bots", expanded: $botsExpanded)
            if botsExpanded {
                if bots.isEmpty {
                    Button { isNewBotPresented = true } label: {
                        Text("No bots yet. Create a bot")
                            .font(.system(size: 12.5))
                            .foregroundColor(Color("TextTertiary"))
                            .padding(.horizontal, 12)
                            .frame(minHeight: 44, alignment: .leading)
                    }
                    .buttonStyle(.plain)
                } else {
                    ForEach(listBots) { botRow($0) }
                }
            }
        }
    }

    /// One bot: avatar with its status dot, name and time, and the last line
    /// of its latest thread (or "Needs you" / "Working" while that holds).
    private func botRow(_ bot: AgentRecord) -> some View {
        let latest = latestThread(bot.id)
        let waiting = store.waitingCount(for: bot.id) > 0 || BotStatusStore.shared.needsAttention(for: bot.id)
        let working = !waiting && (store.workingCount(for: bot.id) > 0 || BotStatusStore.shared.isWorking(for: bot.id))
        let line = latest.map { $0.statusLine?.nilIfBlank ?? $0.title } ?? bot.botTagline
        return Button { onOpenBot(bot.id) } label: {
            HStack(spacing: 12) {
                LiveBotStatusAvatar(bot: bot, size: 46)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 4) {
                        Text(bot.botDisplayName)
                            .font(.system(size: 15, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        Spacer(minLength: 4)
                        if let at = latest?.lastActivityAt {
                            Text(BotTime.short(at))
                                .font(.system(size: 11))
                                .monospacedDigit()
                                .foregroundColor(Color("TextTertiary"))
                        }
                    }
                    HStack(spacing: 4) {
                        if waiting {
                            Text("Needs you").font(.system(size: 12.5, weight: .medium)).foregroundColor(Theme.statusWarning)
                        } else if working {
                            Text("Working").font(.system(size: 12.5, weight: .medium)).foregroundColor(Theme.statusSuccess)
                        } else {
                            Text(line)
                                .font(.system(size: 12.5))
                                .foregroundColor(Color("TextSecondary"))
                                .lineLimit(1)
                        }
                        Spacer(minLength: 0)
                    }
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(bot.botDisplayName + (waiting ? ", needs you" : working ? ", working" : ""))
    }

    /// Group chats sit at the bottom, above the footer; about three rows show.
    private var groupsSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
            sectionHeader("Group chats", expanded: $groupsExpanded, onTitle: { onOpen(.groups) },
                          addLabel: "New group chat") { onOpen(.groups) }
            if groupsExpanded, !groups.isEmpty {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(groups) { group in
                            Button { onOpen(.group(group.id)) } label: {
                                HStack(spacing: 10) {
                                    Image(systemName: "person.3")
                                        .font(.system(size: 13))
                                        .foregroundColor(Color("TextSecondary"))
                                        .frame(width: 30, height: 30)
                                        .background(Circle().fill(Color("FillNeutral")))
                                    Text(group.name)
                                        .font(.system(size: 14))
                                        .foregroundColor(Color("TextPrimary"))
                                        .lineLimit(1)
                                    Spacer(minLength: 4)
                                    Text(BotTime.short(group.updatedAt))
                                        .font(.system(size: 11))
                                        .foregroundColor(Color("TextTertiary"))
                                }
                                .padding(.horizontal, 12)
                                .frame(minHeight: 44)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                .frame(maxHeight: 140)
            }
        }
        .padding(.horizontal, 6)
    }

    private func emptyLine(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12.5))
            .foregroundColor(Color("TextTertiary"))
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
    }

    // MARK: - Helpers

    private func latestThread(_ botId: String) -> BotThread? {
        store.threads(for: botId).max { $0.lastActivityAt < $1.lastActivityAt }
    }

    private func createProject() {
        let title = newProjectName.trimmingCharacters(in: .whitespacesAndNewlines)
        newProjectName = ""
        guard !title.isEmpty else { return }
        Task {
            do {
                let id = try await BotModeClient.shared.createBotProject(title: title)
                await store.refreshProjects()
                if let project = store.projects.first(where: { $0.id == id }) { onOpen(.project(project)) }
            } catch {
                railError = "Couldn't create the project: \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }
}

/// "Set up your main bot" (PrimaryBotSetupDialog, phone): A:// as it comes,
/// or one of your bots. The main bot sits at the top of the Bots rail.
struct MainBotSetupSheet: View {
    let bots: [AgentRecord]
    let onDone: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var busyId: String?
    @State private var error: String?

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Text("Your main bot sits at the top of the Bots rail and is your first stop for anything.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                Section {
                    row(id: "primary", title: "A://", subtitle: "Allternit's own main bot") {
                        try await MeetMainBot.setUpPrimary(name: nil, avatar: nil, dress: false,
                                                           bots: (try? await MeetAgentsAPI.roster()) ?? [])
                    }
                }
                if !bots.isEmpty {
                    Section("Or one of your bots") {
                        ForEach(bots) { bot in
                            row(id: bot.id, title: bot.botDisplayName, subtitle: bot.botTagline, bot: bot) {
                                try await MeetMainBot.set(bot.id, in: (try? await MeetAgentsAPI.roster()) ?? [])
                                return bot.id
                            }
                        }
                    }
                }
                if let error {
                    Section { Text(error).font(.footnote).foregroundColor(Theme.statusError) }
                }
            }
            .navigationTitle("Set up your main bot")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Not now") { dismiss() }.disabled(busyId != nil) }
            }
        }
    }

    private func row(id: String, title: String, subtitle: String, bot: AgentRecord? = nil,
                     action: @escaping () async throws -> String) -> some View {
        Button {
            busyId = id
            error = nil
            Task {
                do {
                    let botId = try await action()
                    busyId = nil
                    dismiss()
                    onDone(botId)
                } catch {
                    busyId = nil
                    self.error = "Couldn't set the main bot: \(FriendlyErrorMessage.from(error.localizedDescription))"
                }
            }
        } label: {
            HStack(spacing: 12) {
                if let bot {
                    BotAvatarView(bot: bot, size: 36)
                } else {
                    Image(systemName: "crown")
                        .frame(width: 36, height: 36)
                        .background(Circle().fill(Color("FillNeutral")))
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.system(size: 15, weight: .medium)).foregroundColor(Color("TextPrimary"))
                    if !subtitle.isEmpty {
                        Text(subtitle).font(.system(size: 12.5)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                    }
                }
                Spacer()
                if busyId == id { ProgressView() }
            }
        }
        .disabled(busyId != nil)
    }
}

private extension String {
    var nilIfBlank: String? {
        let t = trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }
}
