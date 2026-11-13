import AVFoundation
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

            // Behind bot chats, group chats and (when on) Code mode. A look
            // picked on a bot, a room or a Code workspace stays there.
            SettingsSectionHeading(text: "Background")
                .padding(.top, 16)
            StageBackgroundSettingsSection()
                .padding(.bottom, 8)

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

/// Settings → Voice (the web's VoiceSettings panel): where voice runs, and
/// the user's own OpenAI / ElevenLabs keys. Allternit hosts no voice.
struct VoiceSettingsPage: View {
    @ObservedObject private var settings = SettingsStore.shared
    @ObservedObject private var keySync = VoiceKeySync.shared

    private var privacyText: String {
        let base = "Privacy: with voice on this phone, your audio is processed here and isn't sent to Allternit. With your own OpenAI, your audio and the bot's replies go from this phone to OpenAI; with your own ElevenLabs, the bot's replies go to ElevenLabs. Allternit never receives your audio or your key. Either way, what you say becomes a normal message in the thread."
        guard keySync.isEnabled else { return base }
        return base.replacingOccurrences(of: "Allternit never receives your audio or your key.", with: "Allternit never receives your audio.")
            + " Your keys are also saved, encrypted, in your Allternit account so your other devices can use them. Turn this off to delete them from your account."
    }

    private static func routeTitle(_ route: VoiceRoutePreference) -> String {
        switch route {
        case .automatic: return "Automatic (recommended)"
        case .device: return "This device"
        case .openai: return "Your own OpenAI"
        case .elevenlabs: return "Your own ElevenLabs"
        }
    }

    private static func routeDetail(_ route: VoiceRoutePreference) -> String {
        switch route {
        case .automatic: return "Voice runs on this phone. The voice model downloads once; until it's ready, the iPhone's built-in voice is used. Free, and your audio stays on this phone."
        case .device: return "The same, chosen on purpose: your audio stays on this phone."
        case .openai: return "OpenAI Realtime listens and OpenAI speech reads replies, with your API key. OpenAI bills you directly."
        case .elevenlabs: return "An ElevenLabs voice reads replies, with your API key; listening stays on this phone. ElevenLabs bills you directly."
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

            SettingsSectionHeading(text: "Your own provider")
                .padding(.top, 16)
            VoiceOwnProviderSection()

            SettingsSectionHeading(text: "Speech to text")
                .padding(.top, 16)
            SettingsDescription(text: "How speech becomes text when voice runs on this device. Both stay on this device.")
            VStack(spacing: 8) {
                SettingsRadioCard(title: "Automatic (recommended)",
                                  detail: "Uses Apple's on-device speech recognition when this iPhone has it, otherwise Allternit's.",
                                  isSelected: settings.listening == .automatic,
                                  action: { settings.listening = .automatic })
                SettingsRadioCard(title: "Allternit on-device",
                                  detail: "Always uses Allternit's own recognizer (Moonshine, English).",
                                  isSelected: settings.listening == .allternit,
                                  action: { settings.listening = .allternit })
            }

            ReplyVoiceSection()
                .padding(.top, 16)

            LiveModeLookSection(inset: 0)
                .padding(.top, 16)

            Text(privacyText)
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 16)
        }
        // Account sync (no-op while off): newer copy wins on open and foreground.
        .task { await keySync.sync() }
        .onReceive(NotificationCenter.default.publisher(for: UIApplication.willEnterForegroundNotification)) { _ in
            Task { await keySync.sync() }
        }
    }
}

// MARK: - Voice › Your own provider

/// One row per vendor: Connect opens the vendor's key page in a Safari
/// sheet; when it closes, a card offers Paste (no paste prompt) or typing.
/// The key is checked with the vendor, saved in the Keychain, and Where
/// voice runs switches to it. Plus the QR handoff from a computer, the
/// ElevenLabs voice picker, and "Save to my Allternit account".
private struct VoiceOwnProviderSection: View {
    @ObservedObject private var keySync = VoiceKeySync.shared
    @ObservedObject private var auth = AuthManager.shared
    @State private var isScanning = false

    var body: some View {
        let _ = keySync.revision
        VStack(alignment: .leading, spacing: 12) {
            SettingsDescription(text: keySync.isEnabled
                ? "Optional. For a different voice, connect your own account. Your key is kept on this phone, in the Keychain, and in your Allternit account, encrypted. Voice calls go straight to that company. You pay them directly."
                : "Optional. For a different voice, connect your own account. Your key is kept only on this phone, in the Keychain, and goes straight to that company, never to Allternit. You pay them directly.")
            ForEach(VoiceVendor.allCases) { vendor in
                VoiceVendorRow(vendor: vendor)
            }
            if VoiceProviderKeys.value(.elevenlabs) != nil {
                ElevenLabsVoicePicker()
            }

            VStack(alignment: .leading, spacing: 6) {
                Button {
                    isScanning = true
                } label: {
                    Label("Scan from my computer", systemImage: "qrcode.viewfinder")
                }
                .buttonStyle(SettingsQuietButtonStyle())
                Text("Already set up on ai.allternit.com or Allternit Desktop? Show the QR code there and scan it. The key goes from screen to camera, not through Allternit.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }

            SettingsSwitchRow(
                title: "Save to my Allternit account",
                detail: auth.isSignedIn
                    ? "Off by default. Your other signed-in devices get your keys."
                    : "Sign in to Allternit to save keys to your account.",
                isOn: Binding(get: { keySync.isEnabled },
                              set: { on in Task { await keySync.setEnabled(on) } }))
                .disabled(!auth.isSignedIn || keySync.isBusy)
            if let error = keySync.lastError {
                SettingsErrorBanner(message: error)
            }
            Text("Disconnect removes a key from this phone only. To cut a key off everywhere, revoke it in your OpenAI or ElevenLabs account.")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        .sheet(isPresented: $isScanning) { VoiceKeyHandoffSheet() }
    }
}

/// OpenAI or ElevenLabs: Connect → vendor page → Paste → checked → Connected.
private struct VoiceVendorRow: View {
    let vendor: VoiceVendor
    @ObservedObject private var keySync = VoiceKeySync.shared
    @State private var safariURL: IdentifiableURL? = nil
    @State private var isAddingKey = false
    @State private var isTyping = false
    @State private var draft = ""
    @State private var isChecking = false
    @State private var error: String? = nil

    var body: some View {
        let _ = keySync.revision
        let key = VoiceProviderKeys.value(vendor.keyField)
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .center, spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(vendor.name)
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    if key != nil {
                        (Text("● ").foregroundColor(Theme.statusSuccess)
                         + Text("Connected · \(VoiceProviderKeys.label(vendor) ?? "Saved")").foregroundColor(Color("TextSecondary")))
                            .font(.system(size: 13))
                            .lineLimit(2)
                    } else {
                        Text("Not connected")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                Spacer(minLength: 8)
                if key != nil {
                    Button("Disconnect") {
                        keySync.disconnect(vendor)
                        error = nil
                    }
                    .buttonStyle(SettingsQuietButtonStyle())
                    .accessibilityLabel("Disconnect \(vendor.name)")
                } else if !isAddingKey {
                    Button("Connect") {
                        error = nil
                        safariURL = IdentifiableURL(url: vendor.keyPageURL)
                    }
                    .buttonStyle(SettingsPrimaryButtonStyle())
                    .accessibilityLabel("Connect \(vendor.name)")
                }
            }
            if key == nil && isAddingKey {
                addKeyCard
            }
            if let error {
                SettingsErrorBanner(message: error)
            }
        }
        .padding(14)
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .sheet(item: $safariURL, onDismiss: { isAddingKey = true }) { item in
            SafariView(url: item.url).ignoresSafeArea()
        }
    }

    private var addKeyCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Add your \(vendor.name) key")
                .font(.system(size: 15, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
            Text("Copy the key on \(vendor.name)'s page, then paste it here.")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            if isChecking {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Checking your key with \(vendor.name)…")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextSecondary"))
                }
            } else {
                HStack(spacing: 8) {
                    // System paste button: no "Allow Paste" prompt.
                    PasteButton(payloadType: String.self) { strings in
                        let pasted = strings.first ?? ""
                        Task { @MainActor in await connect(pasted) }
                    }
                    .buttonBorderShape(.roundedRectangle(radius: 8))
                    .labelStyle(.titleAndIcon)
                    .tint(Color("AccentPrimary"))
                    if !isTyping {
                        Button("Type it instead") { isTyping = true }
                            .buttonStyle(SettingsQuietButtonStyle())
                    }
                    Spacer(minLength: 0)
                }
                if isTyping {
                    HStack(spacing: 8) {
                        SecureField("Your \(vendor.name) key", text: $draft)
                            .font(.system(size: 14))
                            .autocorrectionDisabled()
                            .textInputAutocapitalization(.never)
                            .padding(.horizontal, 12)
                            .frame(minHeight: 36)
                            .background(Color("BgPrimary"))
                            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                            .accessibilityLabel("\(vendor.name) API key")
                            .onSubmit { Task { await connect(draft) } }
                        Button("Save") { Task { await connect(draft) } }
                            .buttonStyle(SettingsPrimaryButtonStyle())
                            .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
                Button("Cancel") {
                    isAddingKey = false
                    isTyping = false
                    draft = ""
                    error = nil
                }
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("FillNeutral"))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private func connect(_ raw: String) async {
        let key = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else {
            error = "The clipboard has no text. Copy your \(vendor.name) key and try again."
            return
        }
        error = nil
        isChecking = true
        let result = await VoiceVendorClient().checkKey(vendor, key: key)
        isChecking = false
        if case .rejected(let message) = result {
            error = message
            return
        }
        // Offline: keep the key; voice reports a bad key when it's used.
        VoiceProviderKeys.store(vendor, apiKey: key, label: result.savedLabel ?? "Saved, not checked")
        // A Connect on this phone picks that vendor (an account pull never does).
        SettingsStore.shared.voiceRoute = vendor.route
        isAddingKey = false
        isTyping = false
        draft = ""
        keySync.localChanged(vendor)
    }
}

/// Plays ElevenLabs `preview_url` clips.
@MainActor
private final class VoicePreviewPlayer: ObservableObject {
    @Published private(set) var playingId: String?
    private var player: AVPlayer?

    func toggle(_ voice: ElevenLabsVoice) {
        if playingId == voice.id {
            stop()
            return
        }
        guard let url = voice.previewURL else { return }
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .default)
        let player = AVPlayer(url: url)
        self.player = player
        playingId = voice.id
        player.play()
    }

    func stop() {
        player?.pause()
        player = nil
        playingId = nil
    }
}

/// Your ElevenLabs voice: the account's voices (tap to choose, ▶ to hear),
/// with "Enter voice ID" kept for voices the list can't show.
private struct ElevenLabsVoicePicker: View {
    @ObservedObject private var keySync = VoiceKeySync.shared
    @StateObject private var player = VoicePreviewPlayer()
    @State private var voices: [ElevenLabsVoice] = []
    @State private var isLoading = false
    @State private var loadError: String? = nil
    @State private var isEnteringId = false
    @State private var idDraft = ""

    private var selectedId: String {
        VoiceProviderKeys.value(.elevenlabsVoiceId) ?? VoiceProviderKeys.defaultElevenLabsVoice
    }

    var body: some View {
        let _ = keySync.revision
        VStack(alignment: .leading, spacing: 8) {
            Text("ElevenLabs voice")
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
            if isLoading && voices.isEmpty {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Loading your voices…").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                }
            } else if !voices.isEmpty {
                VStack(spacing: 0) {
                    ForEach(Array(voices.enumerated()), id: \.element.id) { index, voice in
                        if index > 0 { PhoneCardDivider() }
                        voiceRow(voice)
                    }
                }
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            }
            if let loadError {
                Text(loadError)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            Button(isEnteringId ? "Hide voice ID" : "Enter voice ID") { isEnteringId.toggle() }
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
            if isEnteringId {
                HStack(spacing: 8) {
                    TextField("\(selectedId)", text: $idDraft)
                        .font(.system(size: 14))
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                        .padding(.horizontal, 12)
                        .frame(minHeight: 36)
                        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                        .accessibilityLabel("ElevenLabs voice ID")
                    Button("Save") {
                        choose(idDraft)
                        idDraft = ""
                    }
                    .buttonStyle(SettingsQuietButtonStyle())
                    .disabled(idDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
        .task(id: VoiceProviderKeys.value(.elevenlabs)) { await load() }
        .onReceive(NotificationCenter.default.publisher(for: .AVPlayerItemDidPlayToEndTime)) { _ in player.stop() }
        .onDisappear { player.stop() }
    }

    private func voiceRow(_ voice: ElevenLabsVoice) -> some View {
        let isSelected = voice.id == selectedId
        return HStack(spacing: 4) {
            Button { choose(voice.id) } label: {
                HStack(spacing: 10) {
                    Image(systemName: "checkmark")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .opacity(isSelected ? 1 : 0)
                        .frame(width: 16)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(voice.name)
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextPrimary"))
                        if let category = voice.category, !category.isEmpty {
                            Text(category.capitalized)
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(isSelected ? [.isSelected, .isButton] : .isButton)
            if voice.previewURL != nil {
                Button { player.toggle(voice) } label: {
                    Image(systemName: player.playingId == voice.id ? "stop.fill" : "play.fill")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 36, height: 36)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(player.playingId == voice.id ? "Stop \(voice.name)" : "Play \(voice.name)")
            }
        }
        .padding(.leading, 12)
        .padding(.trailing, 4)
        .frame(minHeight: 44)
    }

    private func choose(_ id: String) {
        let id = id.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !id.isEmpty else { return }
        VoiceProviderKeys.setVoice(id)
        keySync.localChanged(.elevenlabs)
    }

    private func load() async {
        guard let key = VoiceProviderKeys.value(.elevenlabs) else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            voices = try await VoiceVendorClient().listVoices(key: key)
            loadError = voices.isEmpty ? "No voices on this ElevenLabs account yet. Enter a voice ID instead." : nil
        } catch {
            voices = []
            loadError = error.localizedDescription
            isEnteringId = true
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
    @Environment(\.openURL) private var openURL

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
                    Button("Rate Allternit") {
                        if let url = URL(string: "https://apps.apple.com/app/id6828841999?action=write-review") { openURL(url) }
                    }
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
                    .buttonStyle(.plain)
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
