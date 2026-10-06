import SwiftUI

/// Speech-locale options for the Voice section (Claude parity: a short list
/// of common locales, not the full CLDR set). Consumed by dictation
/// (DictationController builds its SFSpeechRecognizer from it) — persisted
/// here so the choice survives; nil in SettingsStore means System default.
enum SpeechLanguage: String, CaseIterable, Sendable {
    case englishUS = "en-US"
    case englishUK = "en-GB"
    case spanish = "es-ES"
    case french = "fr-FR"
    case german = "de-DE"
    case italian = "it-IT"
    case portuguese = "pt-BR"
    case japanese = "ja-JP"

    var label: String {
        switch self {
        case .englishUS: return "English (US)"
        case .englishUK: return "English (UK)"
        case .spanish: return "Español"
        case .french: return "Français"
        case .german: return "Deutsch"
        case .italian: return "Italiano"
        case .portuguese: return "Português (BR)"
        case .japanese: return "日本語"
        }
    }
}

/// Voice-mode interaction style (Phase 7b, Claude parity): hands-free
/// auto-listens after each spoken reply; push-to-talk gates recognition to
/// a hold gesture on the voice-mode mic button.
enum VoiceInteractionMode: String, CaseIterable, Sendable {
    case handsFree
    case pushToTalk

    var label: String {
        switch self {
        case .handsFree: return "Hands free"
        case .pushToTalk: return "Push to talk"
        }
    }
}

/// "Where voice runs" — which engine serves a voice conversation.
/// Automatic tries Cloud Voice first and falls back to this device.
enum VoiceRoutePreference: String, CaseIterable, Sendable {
    case automatic
    case device
    case cloud

    var label: String {
        switch self {
        case .automatic: return "Automatic"
        case .device: return "This device"
        case .cloud: return "Cloud"
        }
    }

    var detail: String {
        switch self {
        case .automatic: return "Cloud Voice when you have minutes, otherwise this device"
        case .device: return "Your speech is transcribed on this phone; replies use the system voice"
        case .cloud: return "Allternit Cloud Voice: natural voices, uses your Cloud Voice minutes"
        }
    }
}

/// App-wide settings behind the sidebar gear (Phase 4 settings hub).
/// Persisted app-wide (UserDefaults), mirroring ToolOptionsStore's pattern.
///
/// These are UI-persisted flags. Where a flag has a live consumer it is
/// wired there:
/// - `artifactsEnabled` gates inline artifact cards in MessageRow.
/// - `webSearchDefault` seeds ToolOptionsStore.webSearch when the composer
///   toggle hasn't been touched yet (sent as `metadata.tools` on send).
/// The rest await their consumers (code execution, model switching on flag,
/// memory generation, voice output speed, training opt-out) and are marked at
/// their use sites in SettingsView.
@MainActor
final class SettingsStore: ObservableObject {
    static let shared = SettingsStore()

    // MARK: - Capabilities

    /// "Artifacts" — render inline artifact cards in chat.
    @Published var artifactsEnabled: Bool {
        didSet { defaults.set(artifactsEnabled, forKey: Keys.artifactsEnabled) }
    }
    /// "Code execution and file creation" — no consumer wired yet.
    @Published var codeExecutionEnabled: Bool {
        didSet { defaults.set(codeExecutionEnabled, forKey: Keys.codeExecutionEnabled) }
    }
    /// "Web search" — seeds the composer "+" sheet's Web search toggle until
    /// the user overrides it there (see ToolOptionsStore.init).
    @Published var webSearchDefault: Bool {
        didSet { defaults.set(webSearchDefault, forKey: Keys.webSearchDefault) }
    }
    /// "Switch models when a message is flagged" — no consumer wired yet.
    @Published var switchModelsOnFlag: Bool {
        didSet { defaults.set(switchModelsOnFlag, forKey: Keys.switchModelsOnFlag) }
    }

    // MARK: - Memory

    /// "Generate memory from chat history" — persisted locally; the memory
    /// consolidation pipeline consumes it in a later phase.
    @Published var generateMemoryFromHistory: Bool {
        didSet { defaults.set(generateMemoryFromHistory, forKey: Keys.generateMemoryFromHistory) }
    }

    // MARK: - Voice

    /// Dictation locale — consumed by DictationController when a dictation
    /// session starts. nil = System default (follows Locale.current, which
    /// is also the first-run default; the onboarding sheet offers the same
    /// choice and writes this same key).
    @Published var speechLanguage: SpeechLanguage? {
        didSet { defaults.set(speechLanguage?.rawValue, forKey: Keys.speechLanguage) }
    }
    /// Playback/dictation speed — consumed by SpeechSpeaker's utterances
    /// (read-aloud + voice mode replies).
    @Published var speechSpeed: Double {
        didSet { defaults.set(speechSpeed, forKey: Keys.speechSpeed) }
    }
    /// Voice-mode picker selection (Phase 7b): an on-device
    /// AVSpeechSynthesisVoice identifier from VoiceSettingsSheet's list.
    /// Consumed by SpeechSpeaker; nil = system default voice.
    @Published var voiceIdentifier: String? {
        didSet { defaults.set(voiceIdentifier, forKey: Keys.voiceIdentifier) }
    }
    /// Voice-mode interaction style — consumed by VoiceModeViewModel.
    @Published var voiceInteractionMode: VoiceInteractionMode {
        didSet { defaults.set(voiceInteractionMode.rawValue, forKey: Keys.voiceInteractionMode) }
    }

    /// Where voice runs (Automatic / This device / Cloud).
    @Published var voiceRoute: VoiceRoutePreference {
        didSet { defaults.set(voiceRoute.rawValue, forKey: Keys.voiceRoute) }
    }
    /// Cloud Voice voice id (Kokoro, e.g. `af_heart`) — used when the
    /// session runs on Allternit Cloud.
    @Published var cloudVoiceId: String {
        didSet { defaults.set(cloudVoiceId, forKey: Keys.cloudVoiceId) }
    }

    // MARK: - Data controls

    /// "Improve the model for everyone" (training opt-in) — local only; the
    /// backend has no preference endpoint yet.
    @Published var improveModel: Bool {
        didSet { defaults.set(improveModel, forKey: Keys.improveModel) }
    }

    // MARK: - Platform (Settings > Platform parity)

    /// General: display language — mirrors web `general.language`.
    @Published var displayLanguage: String {
        didSet { defaults.set(displayLanguage, forKey: Keys.displayLanguage) }
    }
    /// General: timezone — mirrors web `general.timezone`.
    @Published var timezone: String {
        didSet { defaults.set(timezone, forKey: Keys.timezone) }
    }
    /// General: show system/agent messages in the chat feed.
    @Published var showSystemMessages: Bool {
        didSet { defaults.set(showSystemMessages, forKey: Keys.showSystemMessages) }
    }
    /// General: telemetry opt-in.
    @Published var enableTelemetry: Bool {
        didSet { defaults.set(enableTelemetry, forKey: Keys.enableTelemetry) }
    }
    /// General: auto-save drafts.
    @Published var autoSave: Bool {
        didSet { defaults.set(autoSave, forKey: Keys.autoSave) }
    }

    /// Appearance: compact UI density.
    @Published var compactDensity: Bool {
        didSet { defaults.set(compactDensity, forKey: Keys.compactDensity) }
    }
    /// Appearance: show labels under sidebar tab icons.
    @Published var showSidebarLabels: Bool {
        didSet { defaults.set(showSidebarLabels, forKey: Keys.showSidebarLabels) }
    }

    /// Models: streaming responses toggle.
    @Published var streamingEnabled: Bool {
        didSet { defaults.set(streamingEnabled, forKey: Keys.streamingEnabled) }
    }

    /// Dispatch: cowork dispatch toggle.
    @Published var dispatchEnabled: Bool {
        didSet { defaults.set(dispatchEnabled, forKey: Keys.dispatchEnabled) }
    }

    // MARK: - Products (Settings > Products parity)

    /// Gizzi Code: bypass desktop permission checks (developer-only).
    @Published var gizziBypassPermissions: Bool {
        didSet { defaults.set(gizziBypassPermissions, forKey: Keys.gizziBypassPermissions) }
    }
    /// Gizzi Code: draw-attention notifications.
    @Published var gizziDrawAttentionNotifications: Bool {
        didSet { defaults.set(gizziDrawAttentionNotifications, forKey: Keys.gizziDrawAttentionNotifications) }
    }
    /// Gizzi Code: enable browser tools.
    @Published var gizziBrowserTools: Bool {
        didSet { defaults.set(gizziBrowserTools, forKey: Keys.gizziBrowserTools) }
    }
    /// Gizzi Code: auto-create pull requests.
    @Published var gizziAutoCreatePRs: Bool {
        didSet { defaults.set(gizziAutoCreatePRs, forKey: Keys.gizziAutoCreatePRs) }
    }
    /// Gizzi Code: autofix failing PRs.
    @Published var gizziAutofixPRs: Bool {
        didSet { defaults.set(gizziAutofixPRs, forKey: Keys.gizziAutofixPRs) }
    }

    /// Extensions: auto-update marketplace/sidecar extensions.
    @Published var extensionsAutoUpdate: Bool {
        didSet { defaults.set(extensionsAutoUpdate, forKey: Keys.extensionsAutoUpdate) }
    }
    /// Extensions: use built-in Node runtime.
    @Published var extensionsUseBuiltinNode: Bool {
        didSet { defaults.set(extensionsUseBuiltinNode, forKey: Keys.extensionsUseBuiltinNode) }
    }

    /// Voice speed steps offered in the picker.
    static let speechSpeeds: [Double] = [0.75, 1.0, 1.25, 1.5]

    /// Language choices offered in General settings.
    static let displayLanguages = ["English", "Español", "Français", "Deutsch", "日本語"]
    /// Timezone choices offered in General settings.
    static let timezones = ["UTC", "America/New_York", "America/Los_Angeles", "Europe/London", "Europe/Paris", "Asia/Tokyo"]

    private let defaults: UserDefaults

    private enum Keys {
        static let artifactsEnabled = "allternit-settings-artifacts"
        static let codeExecutionEnabled = "allternit-settings-code-execution"
        static let webSearchDefault = "allternit-settings-web-search-default"
        static let switchModelsOnFlag = "allternit-settings-switch-models-on-flag"
        static let generateMemoryFromHistory = "allternit-settings-generate-memory"
        static let speechLanguage = "allternit-settings-speech-language"
        static let speechSpeed = "allternit-settings-speech-speed"
        static let voiceIdentifier = "allternit-settings-voice-identifier"
        static let voiceInteractionMode = "allternit-settings-voice-interaction-mode"
        static let voiceRoute = "allternit-settings-voice-route"
        static let cloudVoiceId = "allternit-settings-cloud-voice-id"
        static let improveModel = "allternit-settings-improve-model"
        static let displayLanguage = "allternit-settings-display-language"
        static let timezone = "allternit-settings-timezone"
        static let showSystemMessages = "allternit-settings-show-system-messages"
        static let enableTelemetry = "allternit-settings-enable-telemetry"
        static let autoSave = "allternit-settings-auto-save"
        static let compactDensity = "allternit-settings-compact-density"
        static let showSidebarLabels = "allternit-settings-show-sidebar-labels"
        static let streamingEnabled = "allternit-settings-streaming-enabled"
        static let dispatchEnabled = "allternit-settings-dispatch-enabled"
        static let gizziBypassPermissions = "allternit-settings-gizzi-bypass-permissions"
        static let gizziDrawAttentionNotifications = "allternit-settings-gizzi-draw-attention"
        static let gizziBrowserTools = "allternit-settings-gizzi-browser-tools"
        static let gizziAutoCreatePRs = "allternit-settings-gizzi-auto-prs"
        static let gizziAutofixPRs = "allternit-settings-gizzi-autofix-prs"
        static let extensionsAutoUpdate = "allternit-settings-extensions-auto-update"
        static let extensionsUseBuiltinNode = "allternit-settings-extensions-builtin-node"
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        // `object(forKey:)` distinguishes "never set" from an explicit false,
        // so first-run defaults can be true without overriding user choices.
        self.artifactsEnabled = defaults.object(forKey: Keys.artifactsEnabled) as? Bool ?? true
        self.codeExecutionEnabled = defaults.object(forKey: Keys.codeExecutionEnabled) as? Bool ?? false
        self.webSearchDefault = defaults.object(forKey: Keys.webSearchDefault) as? Bool ?? true
        self.switchModelsOnFlag = defaults.object(forKey: Keys.switchModelsOnFlag) as? Bool ?? false
        self.generateMemoryFromHistory = defaults.object(forKey: Keys.generateMemoryFromHistory) as? Bool ?? false
        // nil (never chosen) = System default — matches dictation's
        // pre-setting behavior (Locale.current).
        self.speechLanguage = defaults.string(forKey: Keys.speechLanguage)
            .flatMap(SpeechLanguage.init(rawValue:))
        self.speechSpeed = defaults.object(forKey: Keys.speechSpeed) as? Double ?? 1.0
        self.voiceIdentifier = defaults.string(forKey: Keys.voiceIdentifier)
        self.voiceInteractionMode = defaults.string(forKey: Keys.voiceInteractionMode)
            .flatMap(VoiceInteractionMode.init(rawValue:)) ?? .handsFree
        self.voiceRoute = defaults.string(forKey: Keys.voiceRoute)
            .flatMap(VoiceRoutePreference.init(rawValue:)) ?? .automatic
        self.cloudVoiceId = defaults.string(forKey: Keys.cloudVoiceId) ?? "af_heart"
        self.improveModel = defaults.object(forKey: Keys.improveModel) as? Bool ?? true
        self.displayLanguage = defaults.string(forKey: Keys.displayLanguage) ?? "English"
        self.timezone = defaults.string(forKey: Keys.timezone) ?? "UTC"
        self.showSystemMessages = defaults.object(forKey: Keys.showSystemMessages) as? Bool ?? true
        self.enableTelemetry = defaults.object(forKey: Keys.enableTelemetry) as? Bool ?? true
        self.autoSave = defaults.object(forKey: Keys.autoSave) as? Bool ?? true
        self.compactDensity = defaults.object(forKey: Keys.compactDensity) as? Bool ?? false
        self.showSidebarLabels = defaults.object(forKey: Keys.showSidebarLabels) as? Bool ?? true
        self.streamingEnabled = defaults.object(forKey: Keys.streamingEnabled) as? Bool ?? true
        self.dispatchEnabled = defaults.object(forKey: Keys.dispatchEnabled) as? Bool ?? false
        self.gizziBypassPermissions = defaults.object(forKey: Keys.gizziBypassPermissions) as? Bool ?? false
        self.gizziDrawAttentionNotifications = defaults.object(forKey: Keys.gizziDrawAttentionNotifications) as? Bool ?? true
        self.gizziBrowserTools = defaults.object(forKey: Keys.gizziBrowserTools) as? Bool ?? true
        self.gizziAutoCreatePRs = defaults.object(forKey: Keys.gizziAutoCreatePRs) as? Bool ?? false
        self.gizziAutofixPRs = defaults.object(forKey: Keys.gizziAutofixPRs) as? Bool ?? true
        self.extensionsAutoUpdate = defaults.object(forKey: Keys.extensionsAutoUpdate) as? Bool ?? true
        self.extensionsUseBuiltinNode = defaults.object(forKey: Keys.extensionsUseBuiltinNode) as? Bool ?? true
    }
}
