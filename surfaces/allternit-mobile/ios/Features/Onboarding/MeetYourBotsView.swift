import SwiftUI

/// Whether the person has seen "Meet your bots". One flag across every
/// surface, like the web (allternit-ai `src/lib/bots/bots-intro.ts`): the
/// account flag `botsIntroSeenAt` (cloud-api `GET/PUT /api/v1/me/flags`),
/// with a local copy so the card never flashes back while the account loads
/// or when offline.
@MainActor
final class BotsIntroStore: ObservableObject {
    static let shared = BotsIntroStore()

    nonisolated static let flagKey = "botsIntroSeenAt"
    private static let localKey = "allternit-bots-intro-seen"

    /// True while the intro should be on screen.
    @Published var isPresented = false

    private var checkedThisLaunch = false

    private init() {}

    var seenLocally: Bool { UserDefaults.standard.string(forKey: Self.localKey) != nil }

    /// First workspace appearance: show the intro once per account.
    func checkOnLaunch() {
        guard !checkedThisLaunch else { return }
        checkedThisLaunch = true
        guard AuthManager.shared.isSignedIn else { return }
        if seenLocally {
            // Seen here before the account knew: tell the account.
            Task { await pushLocalToAccountIfNeeded() }
            return
        }
        let key = Self.flagKey
        Task {
            // A slow read must not hold the decision: 3 s, like the web.
            let seen = await withTaskGroup(of: Bool?.self) { group -> Bool? in
                group.addTask {
                    do { return try await AccountFlagsClient.shared.string(key) != nil } catch { return nil }
                }
                group.addTask {
                    try? await Task.sleep(nanoseconds: 3_000_000_000)
                    return nil
                }
                let first = await group.next() ?? nil
                group.cancelAll()
                return first
            }
            if seen == true {
                UserDefaults.standard.set(ISO8601DateFormatter().string(from: Date()), forKey: Self.localKey)
            } else {
                isPresented = true
            }
        }
    }

    /// Settings → "Meet your bots": show it again.
    func replay() {
        isPresented = true
    }

    /// Done or Skip: seen here at once, on the account when reachable.
    func markSeen() {
        let at = ISO8601DateFormatter().string(from: Date())
        UserDefaults.standard.set(at, forKey: Self.localKey)
        isPresented = false
        Task { try? await AccountFlagsClient.shared.set(Self.flagKey, to: at) }
    }

    private func pushLocalToAccountIfNeeded() async {
        guard let at = UserDefaults.standard.string(forKey: Self.localKey) else { return }
        do {
            if try await AccountFlagsClient.shared.string(Self.flagKey) == nil {
                try await AccountFlagsClient.shared.set(Self.flagKey, to: at)
            }
        } catch {
            // Offline: the account catches up on a later launch.
        }
    }
}

/// Presents "Meet your bots" over the workspace (attached to MainWorkspaceView).
struct MeetYourBotsHost: ViewModifier {
    @ObservedObject private var intro = BotsIntroStore.shared
    @EnvironmentObject private var modeStore: AppModeStore

    func body(content: Content) -> some View {
        content
            .onAppear { intro.checkOnLaunch() }
            .fullScreenCover(isPresented: $intro.isPresented) {
                MeetYourBotsView(onFinish: { firstJob in
                    intro.markSeen()
                    if let firstJob {
                        modeStore.selectBarItem(.chats)
                        OnboardingStore.shared.pendingPrompt = firstJob
                    }
                })
            }
    }
}

/// "Meet your bots" for the iPhone — the web card's steps (allternit-ai
/// `src/components/bots/meet/`) in a phone-sized flow: who leads, the team
/// (add bots from the catalog), notifications (the moment push permission
/// is asked, with the why on screen), and the first job (fills the Chats
/// composer; nothing is sent until the person sends it).
struct MeetYourBotsView: View {
    let onFinish: (_ firstJob: String?) -> Void

    private enum Step: Int, CaseIterable {
        case start, team, notifications, job
    }

    @StateObject private var hub = AgentHubStore.shared
    @State private var step: Step = .start
    @State private var adding: Set<String> = []
    @State private var addErrors: [String: String] = [:]
    @State private var notificationsOn = false
    @State private var askingNotifications = false
    @State private var jobText = ""

    private static let jobIdeas = [
        "Every morning, brief me on my inbox and calendar",
        "Research three competitors and summarize what they ship",
        "Draft a weekly update from what I worked on",
    ]

    var body: some View {
        VStack(spacing: 0) {
            header
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    switch step {
                    case .start: startStep
                    case .team: teamStep
                    case .notifications: notificationsStep
                    case .job: jobStep
                    }
                }
                .padding(.horizontal, 20)
                .padding(.top, 8)
                .padding(.bottom, 24)
            }
            footer
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .task {
            hub.fetchAgentsIfNeeded()
            hub.fetchTemplatesIfNeeded()
            let status = await PushNotificationManager.shared.authorizationStatus()
            notificationsOn = status == .authorized || status == .provisional
        }
    }

    // MARK: - Chrome

    private var header: some View {
        HStack {
            HStack(spacing: 6) {
                ForEach(Step.allCases, id: \.rawValue) { s in
                    Capsule()
                        .fill(s.rawValue <= step.rawValue ? Color("AccentPrimary") : Color("TextSecondary").opacity(0.25))
                        .frame(width: s == step ? 22 : 8, height: 6)
                }
            }
            .accessibilityLabel("Step \(step.rawValue + 1) of \(Step.allCases.count)")
            Spacer()
            Button("Skip") { onFinish(nil) }
                .font(.subheadline.weight(.medium))
                .foregroundColor(Color("TextSecondary"))
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    private var footer: some View {
        VStack(spacing: 10) {
            Button(action: primaryAction) {
                Text(primaryTitle)
                    .font(.headline)
                    .foregroundColor(.white)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 15)
                    .background(Color("AccentPrimary"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
            }
            .disabled(askingNotifications)
            if step == .notifications && !notificationsOn {
                Button("Not now") { advance() }
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
            if step == .job {
                Button("I'll do this later") { onFinish(nil) }
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 16)
    }

    private var primaryTitle: String {
        switch step {
        case .start: return "Meet the team"
        case .team: return "Continue"
        case .notifications: return notificationsOn ? "Continue" : "Turn on notifications"
        case .job: return jobText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "Go to my bots" : "Start the job"
        }
    }

    private func primaryAction() {
        switch step {
        case .start, .team:
            advance()
        case .notifications:
            if notificationsOn { advance(); return }
            askingNotifications = true
            Task {
                notificationsOn = await PushNotificationManager.shared.requestPermissionAndRegister()
                askingNotifications = false
                advance()
            }
        case .job:
            let text = jobText.trimmingCharacters(in: .whitespacesAndNewlines)
            onFinish(text.isEmpty ? nil : text)
        }
    }

    private func advance() {
        guard let next = Step(rawValue: step.rawValue + 1) else { return }
        withAnimation(.easeOut(duration: 0.22)) { step = next }
    }

    // MARK: - Steps

    private var startStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Meet your bots")
                .font(.system(size: 30, weight: .bold))
                .foregroundColor(Color("TextPrimary"))
            Text("A:// is your main bot. It leads a team of bots that each take one kind of job — research, writing, your inbox, code — and they keep working on your computer while your phone is in your pocket.")
                .font(.body)
                .foregroundColor(Color("TextSecondary"))
            VStack(alignment: .leading, spacing: 12) {
                point("bubble.left.and.bubble.right", "Talk to any bot from here, like a text thread.")
                point("checkmark.shield", "Bots ask before anything that matters. You approve from your phone.")
                point("clock.arrow.circlepath", "Routines run on a schedule, even while you sleep.")
            }
            .padding(.top, 6)
        }
    }

    private func point(_ icon: String, _ text: String) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: icon)
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(Color("AccentPrimary"))
                .frame(width: 32, height: 32)
                .background(Color("AccentPrimary").opacity(0.12))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM, style: .continuous))
            Text(text)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
            Spacer(minLength: 0)
        }
    }

    private var bots: [AgentRecord] { hub.agents.filter(\.isBot) }

    private var catalog: [AgentTemplate] {
        let names = Set(hub.agents.map { $0.name.lowercased() })
        return hub.templates.filter { !names.contains($0.name.lowercased()) }
    }

    private var teamStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Your team")
                .font(.system(size: 26, weight: .bold))
                .foregroundColor(Color("TextPrimary"))

            if hub.isLoading && hub.agents.isEmpty {
                HStack(spacing: 8) { ProgressView(); Text("Loading your bots…").foregroundColor(Color("TextSecondary")) }
            } else if let error = hub.loadError, hub.agents.isEmpty {
                stateCard(
                    "Your bots live on your computer",
                    "We couldn't reach it right now (\(error)). You can add bots later from the Bots tab."
                ) {
                    Button("Try again") { hub.fetchAgentsIfNeeded(force: true) }
                }
            } else if bots.isEmpty {
                Text("No bots yet. Add a few to start — you can change the team any time.")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            } else {
                ForEach(bots) { bot in
                    row(title: bot.botDisplayName, subtitle: bot.description, seed: bot.id) {
                        Image(systemName: "checkmark.circle.fill").foregroundColor(Color("AccentPrimary"))
                    }
                }
            }

            if !catalog.isEmpty {
                Text("Add to the team")
                    .font(.footnote.weight(.semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 6)
                ForEach(catalog.prefix(8)) { template in
                    row(title: template.name, subtitle: addErrors[template.id] ?? template.description, seed: template.id) {
                        if adding.contains(template.id) {
                            ProgressView()
                        } else {
                            Button("Add") { add(template) }
                                .font(.subheadline.weight(.semibold))
                                .foregroundColor(Color("AccentPrimary"))
                        }
                    }
                }
            } else if hub.isLoadingTemplates {
                HStack(spacing: 8) { ProgressView(); Text("Loading bots you can add…").foregroundColor(Color("TextSecondary")) }
            }
        }
    }

    private func add(_ template: AgentTemplate) {
        adding.insert(template.id)
        addErrors[template.id] = nil
        Task {
            do {
                _ = try await hub.createFromTemplate(templateId: template.id)
            } catch {
                addErrors[template.id] = "Couldn't add: \(error.localizedDescription)"
            }
            adding.remove(template.id)
        }
    }

    private func row<Trailing: View>(title: String, subtitle: String?, seed: String, @ViewBuilder trailing: () -> Trailing) -> some View {
        HStack(spacing: 12) {
            AgentIdenticonView(seed: seed, size: 36)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                if let subtitle, !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 8)
            trailing()
        }
        .padding(12)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
    }

    private func stateCard<Action: View>(_ title: String, _ message: String, @ViewBuilder action: () -> Action) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.subheadline.weight(.semibold)).foregroundColor(Color("TextPrimary"))
            Text(message).font(.caption).foregroundColor(Color("TextSecondary"))
            action().font(.subheadline.weight(.semibold)).foregroundColor(Color("AccentPrimary"))
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
    }

    private var notificationsStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            Image(systemName: notificationsOn ? "bell.badge.fill" : "bell.badge")
                .font(.system(size: 30, weight: .medium))
                .foregroundColor(Color("AccentPrimary"))
                .frame(width: 64, height: 64)
                .background(Color("AccentPrimary").opacity(0.12))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG, style: .continuous))
            Text("Know when a bot needs you")
                .font(.system(size: 26, weight: .bold))
                .foregroundColor(Color("TextPrimary"))
            Text(notificationsOn
                 ? "Notifications are on. You'll hear when a bot asks for your OK, finishes a job, or messages you. Pick which ones in Settings."
                 : "Bots work while you're away. We'll tell you when one asks for your OK, finishes a job, or messages you — nothing else. You choose which ones in Settings.")
                .font(.body)
                .foregroundColor(Color("TextSecondary"))
        }
    }

    private var jobStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Give the team its first job")
                .font(.system(size: 26, weight: .bold))
                .foregroundColor(Color("TextPrimary"))
            Text("Write it the way you'd text a person. It goes into your chat for you to send.")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
            TextField("What should your bots do?", text: $jobText, axis: .vertical)
                .lineLimit(3...6)
                .padding(12)
                .background(Color("BgSecondary"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
            ForEach(Self.jobIdeas, id: \.self) { idea in
                Button { jobText = idea } label: {
                    HStack {
                        Text(idea)
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                            .multilineTextAlignment(.leading)
                        Spacer(minLength: 0)
                        Image(systemName: "arrow.up.left")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .padding(12)
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous)
                            .stroke(Color("TextSecondary").opacity(0.2), lineWidth: 1)
                    )
                }
            }
        }
    }
}
