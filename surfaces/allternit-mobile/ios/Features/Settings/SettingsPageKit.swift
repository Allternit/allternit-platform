import SwiftUI

/// Every page the phone Settings list opens, in the PWA's order
/// (allternit-ai src/views/settings/settings.config.ts `SETTINGS_NAV_ITEMS`:
/// id, label, icon, `phone: {section, order}`). SF Symbols stand in for the
/// web's Phosphor icons. `SettingsRoot` says which pages sit on the root.
enum SettingsPageID: String, CaseIterable, Hashable, Identifiable {
    case profile, appearance, signin, organization, privacy, subscriptions, billing, usage
    case setup, notifications, status, voice, memory, responseStyle = "response-style"
    case openModels = "open-models", gizziCode = "gizziio-code", cowork, devices, computers
    case desktopPet = "desktop-pet", extensions, diagnostics, agentRules = "agent-rules", skills
    case subagents, connectors, connectedApps = "connected-apps", twin, plugins, lens
    case models, imageProviders = "image-providers", videoProviders = "video-providers", mcp
    case apiKeys = "api-keys", permissions, remoteControl = "remote-control", infrastructure
    case security, agents, webhooks, about
    case deviceAccess = "device-access"
    case selfHosted = "self-hosted"

    var id: String { rawValue }

    var label: String {
        switch self {
        case .profile: return "Profile"
        case .appearance: return "General"
        case .signin: return "Account"
        case .organization: return "Organization & access"
        case .privacy: return "Privacy"
        case .subscriptions: return "Connections"
        case .billing: return "Billing"
        case .usage: return "Usage"
        case .setup: return "Setup"
        case .notifications: return "Notifications"
        case .status: return "Status"
        case .voice: return "Voice"
        case .memory: return "Memory"
        case .responseStyle: return "Response style"
        case .openModels: return "Open models"
        case .gizziCode: return "Gizzi Code"
        case .cowork: return "Files & devices"
        case .devices: return "Devices"
        case .computers: return "Computers"
        case .desktopPet: return "Desktop pet"
        case .extensions: return "Extensions"
        case .diagnostics: return "Developer"
        case .agentRules: return "Agent rules"
        case .skills: return "Skills"
        case .subagents: return "Subagents"
        case .connectors: return "Connectors"
        case .connectedApps: return "App approvals"
        case .twin: return "Your twin"
        case .plugins: return "Plugins"
        case .lens: return "Lens Context"
        case .models: return "Models"
        case .imageProviders: return "Image providers"
        case .videoProviders: return "Video providers"
        case .mcp: return "MCP"
        case .apiKeys: return "API Keys"
        case .permissions: return "Permissions"
        case .remoteControl: return "Allternit Mobile"
        case .infrastructure: return "Infrastructure"
        case .security: return "Security"
        case .agents: return "Agents"
        case .webhooks: return "Webhooks"
        case .about: return "About"
        case .deviceAccess: return "Device Access"
        case .selfHosted: return "Self-hosted server"
        }
    }

    var systemImage: String {
        switch self {
        case .profile: return "person.crop.circle"
        case .appearance: return "gearshape"
        case .signin: return "person"
        case .organization: return "building.2"
        case .privacy: return "lock"
        case .subscriptions: return "point.3.connected.trianglepath.dotted"
        case .billing: return "creditcard"
        case .usage: return "gauge.with.dots.needle.33percent"
        case .setup: return "checklist"
        case .notifications: return "bell"
        case .status: return "heart.text.square"
        case .voice: return "mic"
        case .memory: return "brain"
        case .responseStyle: return "text.bubble"
        case .openModels: return "lock.open"
        case .gizziCode: return "chevron.left.forwardslash.chevron.right"
        case .cowork: return "briefcase"
        case .devices: return "laptopcomputer.and.iphone"
        case .computers: return "desktopcomputer"
        case .desktopPet: return "pawprint"
        case .extensions: return "puzzlepiece.extension"
        case .diagnostics: return "waveform.path.ecg"
        case .agentRules, .permissions: return "checkmark.shield"
        case .skills: return "sparkles"
        case .subagents: return "rectangle.connected.to.line.below"
        case .connectors, .connectedApps: return "powerplug"
        case .twin: return "person.3"
        case .plugins: return "shippingbox"
        case .lens: return "cylinder.split.1x2"
        case .models: return "cpu"
        case .imageProviders: return "photo"
        case .videoProviders: return "film"
        case .mcp: return "cable.connector"
        case .apiKeys: return "key"
        case .remoteControl: return "iphone"
        case .infrastructure: return "cloud"
        case .security: return "shield"
        case .agents: return "gearshape.2"
        case .webhooks: return "arrow.triangle.branch"
        case .about: return "info.circle"
        case .deviceAccess: return "hand.raised"
        case .selfHosted: return "server.rack"
        }
    }

    /// `phone: {section, order}` from settings.config.ts: where the
    /// Advanced screen lists the page when it is not a root row. Device
    /// Access and Self-hosted server are iPhone-only pages.
    var phone: (section: SettingsAdvancedSection, order: Int) {
        switch self {
        case .profile: return (.account, 1)
        case .billing: return (.account, 2)
        case .usage: return (.account, 3)
        case .notifications: return (.account, 4)
        case .privacy: return (.account, 5)
        case .signin: return (.account, 6)
        case .organization: return (.account, 7)
        case .setup: return (.account, 8)
        case .memory: return (.assistant, 1)
        case .voice: return (.assistant, 2)
        case .appearance: return (.assistant, 3)
        case .responseStyle: return (.assistant, 4)
        case .openModels: return (.assistant, 5)
        case .subscriptions: return (.bots, 1)
        case .agentRules: return (.bots, 2)
        case .skills: return (.bots, 3)
        case .subagents: return (.bots, 4)
        case .connectors: return (.bots, 5)
        case .plugins: return (.bots, 6)
        case .connectedApps: return (.bots, 7)
        case .twin: return (.bots, 8)
        case .lens: return (.bots, 9)
        case .deviceAccess: return (.bots, 10)
        case .computers: return (.computers, 1)
        case .devices: return (.computers, 2)
        case .cowork: return (.computers, 3)
        case .remoteControl: return (.computers, 4)
        case .desktopPet: return (.computers, 5)
        case .extensions: return (.computers, 6)
        case .selfHosted: return (.computers, 7)
        case .gizziCode: return (.developer, 1)
        case .diagnostics: return (.developer, 2)
        case .status: return (.developer, 3)
        case .models: return (.operatorPages, 1)
        case .imageProviders: return (.operatorPages, 2)
        case .videoProviders: return (.operatorPages, 3)
        case .mcp: return (.operatorPages, 4)
        case .apiKeys: return (.operatorPages, 5)
        case .permissions: return (.operatorPages, 6)
        case .infrastructure: return (.operatorPages, 7)
        case .security: return (.operatorPages, 8)
        case .agents: return (.operatorPages, 9)
        case .webhooks: return (.operatorPages, 10)
        case .about: return (.about, 1)
        }
    }
}

// MARK: - Root and Advanced (phone calm I13; web PHONE_SETTINGS_ROOT)

/// Sections of the pushed Advanced screen, in order (web
/// `PHONE_ADVANCED_SECTIONS`).
enum SettingsAdvancedSection: String, CaseIterable, Sendable {
    case account, assistant, bots, computers, developer
    case operatorPages = "operator"
    case about

    var label: String {
        switch self {
        case .account: return "Account"
        case .assistant: return "Assistant"
        case .bots: return "Bots and agents"
        case .computers: return "Computers and devices"
        case .developer: return "Developer"
        case .operatorPages: return "Operator"
        case .about: return "About"
        }
    }
}

/// One row on the Settings root.
enum SettingsRootRow: Hashable, Sendable {
    /// A settings page; `label` renames it on the root only.
    case page(SettingsPageID, label: String? = nil)
    /// The phone's Customize screen (skills, connectors, plugins).
    case customize
    /// Open links in: In app / Safari (iPhone only; web has no such setting).
    case openLinks
    /// Haptic feedback (`Haptics.isEnabled`, key `allternit.haptics`).
    case haptics
    /// Reduce motion (`AppearanceStore.reduceMotion`, key `allternit.reduceMotion`).
    case reduceMotion

    var title: String {
        switch self {
        case .page(let page, let label): return label ?? page.label
        case .customize: return "Customize"
        case .openLinks: return "Open links in"
        case .haptics: return "Haptic feedback"
        case .reduceMotion: return "Reduce motion"
        }
    }
}

/// One block on the Settings root, top to bottom.
enum SettingsRootEntry: Sendable {
    case accounts
    case group(label: String, rows: [SettingsRootRow])
    case appearance
    case advanced
    case logout
}

/// Pushed screens of the Settings stack that are not a `SettingsPageID`.
enum SettingsRoute: Hashable, Sendable {
    case advanced
    case openLinks
}

/// The Settings root (about 17 rows, the Claude iOS order) and the Advanced
/// screen that lists every other page. Mirrors web `PHONE_SETTINGS_ROOT`,
/// `phoneRootPageIds`, `phoneRootRowCount`, `phoneAdvancedItems`,
/// `phoneSettingsLabel`; the App group adds Open links in.
enum SettingsRoot {
    static let entries: [SettingsRootEntry] = [
        .accounts,
        .group(label: "Account", rows: [
            .page(.profile), .page(.billing), .page(.usage), .page(.notifications), .page(.privacy),
        ]),
        .group(label: "Assistant", rows: [
            .customize, .page(.memory), .page(.voice, label: "Voice and dictation"),
        ]),
        .group(label: "Bots and computers", rows: [
            .page(.subscriptions), .page(.computers, label: "Computers and devices"), .page(.agentRules),
        ]),
        .group(label: "App", rows: [.openLinks, .haptics, .reduceMotion]),
        .appearance,
        .advanced,
        .logout,
    ]

    /// Pages with a row on the root.
    static func pageIDs(_ entries: [SettingsRootEntry] = entries) -> [SettingsPageID] {
        entries.flatMap { entry -> [SettingsRootRow] in
            if case .group(_, let rows) = entry { return rows }
            return []
        }
        .compactMap { row in
            if case .page(let page, _) = row { return page }
            return nil
        }
    }

    /// Rows on the root: each group row, plus Accounts, Appearance, Advanced
    /// and Log out as one row each.
    static func rowCount(_ entries: [SettingsRootEntry] = entries) -> Int {
        entries.reduce(0) { count, entry in
            if case .group(_, let rows) = entry { return count + rows.count }
            return count + 1
        }
    }

    /// The pages Advanced lists in one section: every page not on the root,
    /// in phone order.
    static func advancedItems(_ section: SettingsAdvancedSection, entries: [SettingsRootEntry] = entries) -> [SettingsPageID] {
        let onRoot = Set(pageIDs(entries))
        return SettingsPageID.allCases
            .filter { !onRoot.contains($0) && $0.phone.section == section }
            .sorted { $0.phone.order < $1.phone.order }
    }

    /// How many pages the Advanced row says it holds.
    static func advancedPageCount(_ entries: [SettingsRootEntry] = entries) -> Int {
        SettingsAdvancedSection.allCases.reduce(0) { $0 + advancedItems($1, entries: entries).count }
    }

    /// What the phone calls a page: its root label when the root renames it.
    static func label(for page: SettingsPageID, entries: [SettingsRootEntry] = entries) -> String {
        for entry in entries {
            guard case .group(_, let rows) = entry else { continue }
            for row in rows {
                if case .page(let id, let label?) = row, id == page { return label }
            }
        }
        return page.label
    }
}

/// The ⓘ menu's pages (web `PHONE_SETTINGS_INFO_LINKS`, with the iPhone's
/// own Terms / Privacy URLs and the App Store listing's support URL). No
/// Licenses row: the app has no acknowledgements page.
enum SettingsInfoLinks {
    static let terms = StoreViewCopy.termsURL
    static let privacy = StoreViewCopy.privacyURL
    static let support = URL(string: "https://allternit.com/support")!
    static let status = SystemStatusClient.pageURL
}

// MARK: - Phone list pieces (phone.css "Settings (full sheet, grouped cards)")

/// `.phone-settings-label`: the small label above a card.
struct PhoneSettingsLabel: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 14))
            .foregroundColor(Color("TextSecondary"))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(EdgeInsets(top: 18, leading: 6, bottom: 8, trailing: 6))
            .accessibilityAddTraits(.isHeader)
    }
}

/// `.phone-group.phone-settings-card`: rounded neutral card. Rows inside are
/// split by `PhoneCardDivider` (`.phone-row + .phone-row` hairline).
struct PhoneSettingsCard<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(spacing: 0) { content }
            .background(Color("FillNeutral"))
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}

/// The 0.5pt hairline between two rows of a card.
struct PhoneCardDivider: View {
    var body: some View {
        Rectangle().fill(Color("BorderSubtle")).frame(height: 0.5)
    }
}

/// `.phone-row` content: 22pt icon, label, optional value, trailing accessory.
struct PhoneSettingsRowContent<Trailing: View>: View {
    let title: String
    let systemImage: String
    var value: String? = nil
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: systemImage)
                .font(.system(size: 19, weight: .regular))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 22, height: 22)
                .accessibilityHidden(true)
            Text(title)
                .font(.system(size: 16))
                .foregroundColor(Color("TextPrimary"))
                .frame(maxWidth: .infinity, alignment: .leading)
            if let value {
                Text(value)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            trailing
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 50)
        .contentShape(Rectangle())
    }
}

/// The row's ‹›: `CaretRight size 16`, muted.
struct PhoneRowChevron: View {
    var body: some View {
        Image(systemName: "chevron.right")
            .font(.system(size: 14, weight: .regular))
            .foregroundColor(Color("TextSecondary"))
            .accessibilityHidden(true)
    }
}

/// Press highlight for a `.phone-row` (`:active { background: fill }`).
struct PhoneRowButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(configuration.isPressed ? Color("FillNeutral") : Color.clear)
    }
}

/// `.phone-switch`: 50×30 track, 26pt knob; on = the accent.
struct PhoneSwitch: View {
    let isOn: Bool

    var body: some View {
        ZStack(alignment: isOn ? .trailing : .leading) {
            Capsule()
                .fill(isOn ? Color("BrandAccent") : Color("TextPrimary").opacity(0.09))
            Circle()
                .fill(Color.white)
                .shadow(color: .black.opacity(0.25), radius: 1.5, y: 1)
                .frame(width: 26, height: 26)
                .padding(2)
        }
        .frame(width: 50, height: 30)
        .animation(.easeOut(duration: 0.2), value: isOn)
        .accessibilityHidden(true)
    }
}

/// `ToggleRow`: a simple on/off setting switched in place.
struct PhoneToggleRow: View {
    let title: String
    let systemImage: String
    @Binding var isOn: Bool

    var body: some View {
        Button(action: { isOn.toggle() }) {
            PhoneSettingsRowContent(title: title, systemImage: systemImage) {
                PhoneSwitch(isOn: isOn)
            }
        }
        .buttonStyle(PhoneRowButtonStyle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityValue(isOn ? "On" : "Off")
        .accessibilityAddTraits(.isButton)
    }
}

// MARK: - Page pieces (SettingsView variant="phone": the panel in a 16pt gutter)

/// A pushed settings page: scrolls, 16pt gutter, page background.
struct PhoneSettingsPage<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                content
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(EdgeInsets(top: 4, leading: 16, bottom: 32, trailing: 16))
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// `SectionHeading`: a page's section title.
struct SettingsSectionHeading: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 17, weight: .semibold))
            .foregroundColor(Color("TextPrimary"))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 20)
            .padding(.bottom, 12)
            .accessibilityAddTraits(.isHeader)
    }
}

/// The muted sentence under a heading.
struct SettingsDescription: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 14))
            .foregroundColor(Color("TextSecondary"))
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.bottom, 14)
    }
}

/// A bordered settings card (`SettingsCard`).
struct SettingsCardBox<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(18)
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .padding(.bottom, 16)
    }
}

/// `SettingsRow`: label and description left, the control right.
struct SettingsLabeledRow<Control: View>: View {
    let title: String
    var detail: String? = nil
    @ViewBuilder var control: Control

    var body: some View {
        HStack(alignment: .center, spacing: 16) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                if let detail {
                    Text(detail)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            control
        }
        .padding(.vertical, 12)
    }
}

/// A switch row on a page (the web's settings switch, accent when on).
struct SettingsSwitchRow: View {
    let title: String
    var detail: String? = nil
    @Binding var isOn: Bool

    var body: some View {
        SettingsLabeledRow(title: title, detail: detail) {
            Toggle(title, isOn: $isOn)
                .labelsHidden()
                .tint(Color("BrandAccent"))
        }
    }
}

/// The red alert strip (`role="alert"`, status-error tint).
struct SettingsErrorBanner: View {
    let message: String

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.circle")
                .font(.system(size: 15))
            Text(message)
                .font(.system(size: 14))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .foregroundColor(Theme.statusError)
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .background(Theme.statusError.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .padding(.bottom, 12)
        .accessibilityElement(children: .combine)
    }
}

/// One choice in a radio list (Response style, Where voice runs).
struct SettingsRadioCard: View {
    let title: String
    let detail: String?
    let isSelected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(alignment: .center, spacing: 14) {
                ZStack {
                    Circle()
                        .strokeBorder(isSelected ? Color("TextPrimary") : Color("BorderStrong"), lineWidth: isSelected ? 5 : 1)
                        .frame(width: 16, height: 16)
                }
                .frame(width: 20)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    if let detail {
                        Text(detail)
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .background(isSelected ? Color("FillNeutral") : Color.clear)
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(isSelected ? Color("TextPrimary") : Color("BorderSubtle"), lineWidth: 1)
            )
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isSelected ? [.isSelected, .isButton] : .isButton)
    }
}

/// Quiet bordered button (`QUIET_BUTTON_CLASS`).
struct SettingsQuietButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .medium))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 14)
            .frame(minHeight: 36)
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(Color("BorderStrong"), lineWidth: 1)
            )
            .opacity(configuration.isPressed ? 0.6 : 1)
    }
}

/// Primary filled button (`PRIMARY_BUTTON_CLASS`).
struct SettingsPrimaryButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .medium))
            .foregroundColor(Color("OnAccent"))
            .padding(.horizontal, 16)
            .frame(minHeight: 36)
            .background(Color("AccentPrimary").opacity(isEnabled ? 1 : 0.45))
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .opacity(configuration.isPressed ? 0.8 : 1)
    }
}

// MARK: - Pages the phone has no native panel for

/// A page whose panel only runs on the web or Desktop (it needs the runtime's
/// own APIs): the web panel's heading and opening sentence, then a link that
/// opens the same page on ai.allternit.com (`/settings/<id>`, the PWA's
/// settings path deep link).
struct SettingsWebPage: View {
    let page: SettingsPageID
    let intro: String
    /// Follows Settings → App → Open links in (the root handler).
    @Environment(\.openURL) private var openURL

    var body: some View {
        PhoneSettingsPage(title: SettingsRoot.label(for: page)) {
            SettingsSectionHeading(text: SettingsRoot.label(for: page))
            SettingsDescription(text: intro)
            Button("Open on the web") {
                if let url = URL(string: "https://ai.allternit.com/settings/\(page.rawValue)") {
                    openURL(url)
                }
            }
            .buttonStyle(SettingsQuietButtonStyle())
        }
    }
}

/// A page whose web panel is a single sentence (Desktop-only features).
struct SettingsNotePage: View {
    let page: SettingsPageID
    let heading: String
    let note: String

    var body: some View {
        PhoneSettingsPage(title: page.label) {
            SettingsSectionHeading(text: heading)
            SettingsDescription(text: note)
        }
    }
}
