import SwiftUI
import UIKit

// The settings pages the phone list pushes, each laid out like the web panel
// in SettingsView's phone layout (allternit-ai src/views/settings/*): a
// section heading, a muted sentence, then rows or cards in a 16pt gutter.

// MARK: - Profile

/// The Settings → Profile fields on the agent-preferences row
/// (`GET/PUT /api/v1/agent-preferences`, agent-preferences-api.ts
/// `UserProfileFields`). The PUT takes a partial body, so saving the
/// profile leaves response style and instructions alone.
struct ProfileFields: Codable, Equatable, Sendable {
    var fullName: String = ""
    var preferredName: String = ""
    var occupation: String = ""
    var personalPreferences: String = ""

    enum CodingKeys: String, CodingKey {
        case fullName = "full_name"
        case preferredName = "preferred_name"
        case occupation
        case personalPreferences = "personal_preferences"
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        fullName = (try? c.decodeIfPresent(String.self, forKey: .fullName)) ?? ""
        preferredName = (try? c.decodeIfPresent(String.self, forKey: .preferredName)) ?? ""
        occupation = (try? c.decodeIfPresent(String.self, forKey: .occupation)) ?? ""
        personalPreferences = (try? c.decodeIfPresent(String.self, forKey: .personalPreferences)) ?? ""
    }
}

/// Settings → Profile (ProfilePanel.tsx): who you are, for every agent and
/// model, then the plan card.
struct ProfileSettingsPage: View {
    @EnvironmentObject private var authManager: AuthManager
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var usageStore = UsageStore.shared

    @State private var saved = ProfileFields()
    @State private var draft = ProfileFields()
    @State private var loading = true
    @State private var saving = false
    @State private var justSaved = false
    @State private var error: String? = nil

    /// ProfilePanel.tsx OCCUPATIONS.
    private static let occupations = [
        "Founder or operator", "Software engineering", "Design", "Product management", "Research",
        "Marketing or sales", "Writing or content", "Data or analytics", "Student", "Other",
    ]

    var body: some View {
        PhoneSettingsPage(title: "Profile") {
            SettingsSectionHeading(text: "Profile")
            HStack(spacing: 16) {
                AccountAvatar(size: 56)
                VStack(alignment: .leading, spacing: 2) {
                    Text(displayName)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if let email = authManager.accountEmail {
                        Text(email)
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                }
            }
            .padding(.bottom, 8)

            if let error { SettingsErrorBanner(message: error) }

            field("Full name") {
                TextField("", text: $draft.fullName)
                    .textContentType(.name)
                    .modifier(SettingsInputStyle())
            }
            field("What should Allternit call you?") {
                TextField(draft.fullName.split(separator: " ").first.map(String.init) ?? "", text: $draft.preferredName)
                    .textContentType(.nickname)
                    .modifier(SettingsInputStyle())
            }
            field("What best describes your work?") {
                Menu {
                    Picker("What best describes your work?", selection: $draft.occupation) {
                        Text("Choose one").tag("")
                        ForEach(Self.occupations, id: \.self) { Text($0).tag($0) }
                    }
                } label: {
                    HStack {
                        Text(draft.occupation.isEmpty ? "Choose one" : draft.occupation)
                            .foregroundColor(Color("TextPrimary"))
                        Spacer()
                        Image(systemName: "chevron.down")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .modifier(SettingsInputStyle())
                }
            }
            field("Personal preferences",
                  detail: "Every agent and bot reads this, on every model and subscription. How you like answers, what to check first, what never to do.") {
                ZStack(alignment: .topLeading) {
                    if draft.personalPreferences.isEmpty {
                        Text("e.g. Plain, direct answers. Ask before anything that sends money or messages a client.")
                            .font(.system(size: 16))
                            .foregroundColor(Color("TextTertiary"))
                            .padding(.horizontal, 13)
                            .padding(.vertical, 12)
                            .allowsHitTesting(false)
                    }
                    TextEditor(text: $draft.personalPreferences)
                        .font(.system(size: 16))
                        .scrollContentBackground(.hidden)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                }
                .frame(minHeight: 120)
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(Color("BorderStrong"), lineWidth: 1)
                )
            }

            HStack(spacing: 10) {
                Spacer()
                if justSaved && !dirty {
                    Label("Saved", systemImage: "checkmark")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                Button(action: { Task { await save() } }) {
                    if saving {
                        ProgressView().tint(Color("OnAccent"))
                    } else {
                        Text("Save profile")
                    }
                }
                .buttonStyle(SettingsPrimaryButtonStyle())
                .disabled(loading || saving || !dirty)
            }
            .padding(.top, 4)

            SettingsSectionHeading(text: "Your plan")
                .padding(.top, 16)
            SettingsCardBox {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Allternit Cloud")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text(planDetail)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                HStack(spacing: 8) {
                    NavigationLink("Usage", value: SettingsPageID.usage)
                        .buttonStyle(SettingsQuietButtonStyle())
                    NavigationLink("See plans", value: SettingsPageID.billing)
                        .buttonStyle(SettingsQuietButtonStyle())
                }
            }
        }
        .task { await load() }
    }

    private var dirty: Bool { draft != saved }

    private var displayName: String {
        let name = [draft.preferredName, draft.fullName, authManager.fullName ?? ""].first { !$0.isEmpty }
        return name ?? "You"
    }

    private var planDetail: String {
        guard authManager.isSignedIn else { return "Not active on this device. Sign in to use Allternit Cloud." }
        var parts: [String] = []
        if let name = plan.planName { parts.append(name) }
        if let percent = usageStore.percentUsed {
            parts.append("\(max(0, 100 - Int(percent.rounded())))% left · \(usageStore.periodLabel.lowercased())")
        }
        return parts.isEmpty ? "Free" : parts.joined(separator: " · ")
    }

    private func field<Content: View>(_ label: String, detail: String? = nil, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(label)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
            if let detail {
                Text(detail)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            content()
        }
        .padding(.vertical, 8)
    }

    private func friendly(_ error: Error, _ fallback: String) -> String {
        let message = error.localizedDescription
        if message.range(of: "unauthori[sz]ed|401|authorization token", options: [.regularExpression, .caseInsensitive]) != nil {
            return "Sign in to Allternit to save your profile."
        }
        return message.isEmpty ? fallback : FriendlyErrorMessage.from(message)
    }

    @MainActor
    private func load() async {
        defer { loading = false }
        do {
            let fields: ProfileFields = try await APIClient.shared.get(path: "agent-preferences")
            saved = fields
            var initial = fields
            // First visit: start from the account's name rather than a blank field.
            if initial.fullName.isEmpty { initial.fullName = authManager.fullName ?? "" }
            draft = initial
        } catch {
            self.error = friendly(error, "Couldn’t load your profile.")
            if draft.fullName.isEmpty { draft.fullName = authManager.fullName ?? "" }
        }
    }

    @MainActor
    private func save() async {
        saving = true
        error = nil
        defer { saving = false }
        do {
            try await APIClient.shared.put(path: "agent-preferences", body: draft)
            saved = draft
            justSaved = true
        } catch {
            self.error = friendly(error, "Couldn’t save your profile.")
        }
    }
}

/// `SETTINGS_INPUT_CLASS`: 44pt bordered field, 16pt text.
private struct SettingsInputStyle: ViewModifier {
    func body(content: Content) -> some View {
        content
            .font(.system(size: 16))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 13)
            .frame(minHeight: 44)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(Color("BorderStrong"), lineWidth: 1)
            )
    }
}

// MARK: - Response style

/// Settings → Response style (ResponseStylePanel.tsx): the style for every
/// agent chat and the custom instructions, saved to the account
/// (PreferencesStore → `PUT /api/v1/agent-preferences`).
struct ResponseStyleSettingsPage: View {
    @ObservedObject private var preferences = PreferencesStore.shared
    @State private var instructions = ""
    @State private var didSeed = false

    private static func detail(_ style: ResponseStyle) -> String {
        switch style {
        case .concise: return "The answer in one line, then up to 3 bullets."
        case .balanced: return "The answer in one line, then short sections."
        case .detailed: return "The full layout, with reasoning, examples and edge cases."
        case .custom: return "Your instructions below set the length. The layout stays the same."
        }
    }

    var body: some View {
        PhoneSettingsPage(title: "Response style") {
            SettingsSectionHeading(text: "Response style")
            SettingsDescription(text: "Controls how agents respond across every chat. Saved to your account and synced into each agent's workspace automatically.")
            if let error = preferences.saveError {
                SettingsErrorBanner(message: FriendlyErrorMessage.from(error))
            }

            SettingsCardBox {
                Text("Style")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("Pick how much detail agents put into their replies. Every agent lays answers out the same way: the answer first in one line, then a fixed layout for the kind of question (comparison, plan, how-to, troubleshooting, research, status), with sources as links.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
                VStack(spacing: 8) {
                    ForEach(ResponseStyle.allCases, id: \.self) { style in
                        SettingsRadioCard(
                            title: style.label,
                            detail: Self.detail(style),
                            isSelected: preferences.responseStyle == style,
                            action: { preferences.save(style: style, instructions: preferences.customInstructions) }
                        )
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityLabel("Style")
            }

            SettingsCardBox {
                Text("Custom instructions")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("Extra guidance appended to the response style. Applied to every agent chat.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                TextEditor(text: $instructions)
                    .font(.system(size: 16))
                    .scrollContentBackground(.hidden)
                    .padding(6)
                    .frame(minHeight: 140)
                    .overlay(
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .stroke(Color("BorderStrong"), lineWidth: 1)
                    )
                    .accessibilityLabel("Instructions")
                HStack {
                    Text("\(instructions.count) characters · applied to every agent chat")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                    Spacer()
                    Button("Save instructions") {
                        preferences.save(style: preferences.responseStyle,
                                         instructions: instructions.trimmingCharacters(in: .whitespacesAndNewlines))
                    }
                    .buttonStyle(SettingsPrimaryButtonStyle())
                    .disabled(preferences.isSaving || instructions == preferences.customInstructions)
                }
            }
        }
        .task { preferences.fetchIfNeeded() }
        .onAppear { seed() }
        .onChange(of: preferences.isLoaded) { _, _ in seed() }
    }

    private func seed() {
        guard !didSeed, preferences.isLoaded else { return }
        didSeed = true
        instructions = preferences.customInstructions
    }
}

// MARK: - General

/// Settings → General (SettingsView.tsx renderAppearancePanel, phone):
/// Appearance (Theme, Chat font, Reduce motion) and Composer.
struct GeneralSettingsPage: View {
    @ObservedObject private var appearance = AppearanceStore.shared
    @ObservedObject private var settings = SettingsStore.shared

    var body: some View {
        PhoneSettingsPage(title: "General") {
            SettingsSectionHeading(text: "Appearance")
            SettingsLabeledRow(title: "Theme", detail: "Light, dark, or match your system") {
                ThemeSegment()
            }
            SettingsLabeledRow(title: "Chat font", detail: "Font for responses in the transcript") {
                Menu {
                    Picker("Chat font", selection: $appearance.chatFont) {
                        ForEach(AppearanceStore.ChatFont.allCases) { Text($0.label).tag($0) }
                    }
                } label: {
                    HStack(spacing: 6) {
                        Text(appearance.chatFont.label)
                            .foregroundColor(Color("TextPrimary"))
                        Image(systemName: "chevron.down")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .font(.system(size: 15))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .overlay(
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .stroke(Color("BorderStrong"), lineWidth: 1)
                    )
                }
                .accessibilityLabel("Chat font")
            }
            SettingsSwitchRow(title: "Reduce motion",
                              detail: "Turn off animation in streaming responses and other interface elements",
                              isOn: $appearance.reduceMotion)

            SettingsSectionHeading(text: "Composer")
                .padding(.top, 16)
            SettingsSwitchRow(title: "Auto-save chat drafts",
                              detail: "Automatically save your work",
                              isOn: $settings.autoSave)
        }
    }
}

/// The web's three-icon theme switch: System · Light · Dark.
private struct ThemeSegment: View {
    @ObservedObject private var store = AppearanceStore.shared

    private let options: [(AppearanceStore.Mode, String)] = [
        (.system, "desktopcomputer"), (.light, "sun.max"), (.dark, "moon"),
    ]

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options, id: \.0) { option in
                let mode = option.0
                Button(action: { store.theme = mode }) {
                    Image(systemName: option.1)
                        .font(.system(size: 14))
                        .foregroundColor(store.theme == mode ? Color("TextPrimary") : Color("TextSecondary"))
                        .frame(width: 28, height: 28)
                        .background(store.theme == mode ? Color("BgPanel") : Color.clear)
                        .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
                        .shadow(color: .black.opacity(store.theme == mode ? 0.08 : 0), radius: 1, y: 1)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(mode.label)
                .accessibilityAddTraits(store.theme == mode ? [.isSelected, .isButton] : .isButton)
            }
        }
        .padding(3)
        .background(Color("FillNeutral"))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

// MARK: - Voice

/// Settings → Voice (the web's VoiceSettings panel): where voice runs and
/// how your bots sound. The Kokoro voice ids are the ones Cloud Voice speaks
/// with (`SettingsStore.cloudVoiceId`, read by VoiceModeViewModel).
struct VoiceSettingsPage: View {
    @ObservedObject private var settings = SettingsStore.shared

    private struct VoiceOption: Identifiable {
        let name: String
        let accent: String
        let id: String
        init(_ name: String, _ accent: String, _ id: String) {
            self.name = name
            self.accent = accent
            self.id = id
        }
    }

    /// The web's voice list: friendly name, accent, Kokoro id.
    private static let voices: [VoiceOption] = [
        VoiceOption("Steady", "American English", "af_alloy"), VoiceOption("Bright", "American English", "af_aoede"),
        VoiceOption("Warm", "American English", "af_bella"), VoiceOption("Gentle", "American English", "af_heart"),
        VoiceOption("Easy", "American English", "af_jessica"), VoiceOption("Light", "American English", "af_kore"),
        VoiceOption("Soft", "American English", "af_nicole"), VoiceOption("Clear", "American English", "af_nova"),
        VoiceOption("Calm", "American English", "af_river"), VoiceOption("Lively", "American English", "af_sarah"),
        VoiceOption("Airy", "American English", "af_sky"), VoiceOption("Level", "American English", "am_adam"),
        VoiceOption("Crisp", "American English", "am_echo"), VoiceOption("Plain", "American English", "am_eric"),
        VoiceOption("Deep", "American English", "am_fenrir"), VoiceOption("Young", "American English", "am_liam"),
        VoiceOption("Firm", "American English", "am_michael"), VoiceOption("Low", "American English", "am_onyx"),
        VoiceOption("Quick", "American English", "am_puck"), VoiceOption("Gravel", "American English", "am_santa"),
        VoiceOption("Neat", "British English", "bf_alice"), VoiceOption("Mellow", "British English", "bf_emma"),
        VoiceOption("Smooth", "British English", "bf_isabella"), VoiceOption("Mild", "British English", "bf_lily"),
        VoiceOption("Direct", "British English", "bm_daniel"), VoiceOption("Story", "British English", "bm_fable"),
        VoiceOption("Elder", "British English", "bm_george"), VoiceOption("Slow", "British English", "bm_lewis"),
    ]

    private static func routeTitle(_ route: VoiceRoutePreference) -> String {
        switch route {
        case .automatic: return "Automatic (recommended)"
        case .device: return "This device"
        case .cloud: return "Allternit Cloud"
        }
    }

    private static func routeDetail(_ route: VoiceRoutePreference) -> String {
        switch route {
        case .automatic: return "Uses this device when its voice engine is ready and fast enough, otherwise Allternit Cloud. Phones use Cloud unless you choose This device."
        case .device: return "Free on every plan. Your audio stays on this device."
        case .cloud: return "Runs on Allternit servers: easier on battery and slower devices. Uses Cloud Voice minutes."
        }
    }

    var body: some View {
        PhoneSettingsPage(title: "Voice") {
            SettingsSectionHeading(text: "Where voice runs")
            SettingsDescription(text: "Talk to your bots in any session. Voice turns speech into messages and reads replies aloud; the bot, tools and approvals work the same as typing.")
            VStack(spacing: 8) {
                ForEach(VoiceRoutePreference.allCases, id: \.self) { route in
                    SettingsRadioCard(title: Self.routeTitle(route), detail: Self.routeDetail(route),
                                      isSelected: settings.voiceRoute == route,
                                      action: { settings.voiceRoute = route })
                }
            }

            SettingsSectionHeading(text: "Voice")
                .padding(.top, 16)
            SettingsDescription(text: "How your bots sound with Allternit Cloud.")
            VStack(spacing: 8) {
                ForEach(Self.voices) { voice in
                    SettingsRadioCard(title: voice.name, detail: "\(voice.accent) · \(voice.id)",
                                      isSelected: settings.cloudVoiceId == voice.id,
                                      action: { settings.cloudVoiceId = voice.id })
                }
            }

            Text("Privacy: with voice on this device, your audio is processed on this device and isn't sent to Allternit. With Allternit Cloud, audio is sent to Allternit's servers to run the conversation.")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 16)
        }
    }
}

// MARK: - Developer

/// Settings → Developer (the web's diagnostics panel): telemetry and system.
/// Local engines and Factory services run on Desktop.
struct DeveloperSettingsPage: View {
    var body: some View {
        PhoneSettingsPage(title: "Developer") {
            SettingsSectionHeading(text: "Local engines")
            SettingsDescription(text: "Engine status is only available in Allternit Desktop.")
            SettingsSectionHeading(text: "Telemetry & system")
            VStack(spacing: 0) {
                row("App Version", AboutSettingsPage.versionString)
                PhoneCardDivider()
                row("Platform", "iOS \(UIDevice.current.systemVersion)")
                PhoneCardDivider()
                row("Backend Mode", AppConfig.apiBaseURL.host ?? AppConfig.apiBaseURL.absoluteString)
            }
            .overlay(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
        }
    }

    private func row(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).font(.system(size: 14)).foregroundColor(Color("TextPrimary"))
            Spacer()
            Text(value)
                .font(.system(size: 13, design: .monospaced))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .padding(.horizontal, 14)
        .frame(minHeight: 44)
    }
}

// MARK: - Allternit Mobile (mesh)

/// Computers → Allternit Mobile: this phone's node on the Allternit mesh.
/// Start enrolls via the platform (`POST /api/v1/mesh/enroll`) and joins the
/// tailnet; failures show in the Status row.
struct AllternitMobileSettingsPage: View {
    @ObservedObject private var mesh = MeshClient.shared
    #if DEBUG
    /// Pre-auth key entry (DEBUG only) — overrides platform enrollment.
    @AppStorage("mesh-auth-key") private var meshAuthKey = ""
    #endif

    var body: some View {
        PhoneSettingsPage(title: "Allternit Mobile") {
            SettingsSectionHeading(text: "Allternit Mobile")
            SettingsDescription(text: "This phone joins the Allternit fabric as a node so it can reach your own computers directly over the Allternit mesh.")
            #if DEBUG
            SecureField("Pre-auth key (debug)", text: $meshAuthKey)
                .font(.system(.subheadline, design: .monospaced))
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .padding(.bottom, 8)
            #endif
            SettingsLabeledRow(title: "This node") {
                switch mesh.state {
                case .idle:
                    Text("Off").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                case .starting:
                    HStack(spacing: 8) {
                        ProgressView()
                        Text("Joining…").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                    }
                case .up(let meshIP):
                    Text(meshIP)
                        .font(.system(size: 14, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                case .failed:
                    Text("Couldn't connect").font(.system(size: 14)).foregroundColor(Theme.statusError)
                }
            }
            if case .failed(let message) = mesh.state {
                SettingsErrorBanner(message: FriendlyErrorMessage.from(message))
            }
            if case .up = mesh.state {
                Button("Disconnect this phone") { mesh.stop() }
                    .buttonStyle(SettingsQuietButtonStyle())
            } else {
                Button("Connect this phone") { Task { await mesh.startWithPlatformAuth() } }
                    .buttonStyle(SettingsPrimaryButtonStyle())
                    .disabled(mesh.state == .starting)
            }
        }
    }
}

// MARK: - About

/// Settings → About (SettingsView.tsx renderAboutPanel): the tile mark,
/// "Allternit & Coffee", the build, and Terms · Privacy · GitHub.
struct AboutSettingsPage: View {
    @State private var safariURL: IdentifiableURL? = nil

    static var versionString: String {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—"
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—"
        return "\(version) (\(build))"
    }

    private static var buildLine: String {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—"
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—"
        return "Allternit for iPhone \(version) · build \(build)"
    }

    var body: some View {
        PhoneSettingsPage(title: "About") {
            VStack(spacing: 0) {
                LazyVGrid(columns: Array(repeating: GridItem(.fixed(34), spacing: 8), count: 4), spacing: 8) {
                    ForEach(0..<16, id: \.self) { i in
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .fill(Color("BrandAccent"))
                            .opacity(i % 3 == 0 ? 0.3 : i % 2 == 0 ? 0.6 : 1)
                            .frame(width: 34, height: 34)
                    }
                }
                .frame(width: 160)
                .padding(.bottom, 40)
                .accessibilityHidden(true)

                (Text("Allternit & ").foregroundColor(Color("TextPrimary"))
                    + Text("Coffee").foregroundColor(Color("BrandAccent")))
                    .font(.system(size: 30, weight: .semibold))
                    .tracking(-0.5)
                    .padding(.bottom, 8)
                Text(Self.buildLine)
                    .font(.system(size: 13, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))

                HStack(spacing: 24) {
                    link("Terms", "https://allternit.com/terms")
                    link("Privacy", "https://allternit.com/privacy")
                    link("GitHub", "https://github.com/allternit")
                }
                .padding(.top, 40)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 40)
            #if DEBUG
            Text(AppConfig.apiBaseURL.absoluteString)
                .font(.system(size: 12, design: .monospaced))
                .foregroundColor(Color("TextTertiary"))
                .frame(maxWidth: .infinity)
            #endif
        }
        .sheet(item: $safariURL) { item in SafariView(url: item.url) }
    }

    private func link(_ title: String, _ raw: String) -> some View {
        Button(title) { if let url = URL(string: raw) { safariURL = IdentifiableURL(url: url) } }
            .font(.system(size: 13, weight: .medium))
            .foregroundColor(Color("TextSecondary"))
            .buttonStyle(.plain)
    }
}
