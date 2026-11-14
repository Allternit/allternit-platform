import Foundation

// The client catalog for the connect kit: the parts of each connection that
// don't change per account (contract §2). It mirrors the web registry
// (allternit-ai `src/components/connect-kit/registry.ts`) id for id and copy
// for copy. Live state, availability and fallbacks come from the server.

enum ConnGroup: String, CaseIterable, Sendable {
    case brains, computers, phones, tools, voice, apps, channels

    var title: String {
        switch self {
        case .brains: return "Brains"
        case .computers: return "Computers"
        case .phones: return "Phones"
        case .tools: return "Coding tools"
        case .voice: return "Voice"
        case .apps: return "Apps"
        case .channels: return "Channels"
        }
    }

    var blurb: String {
        switch self {
        case .brains: return "The AI that thinks. Use a plan you already pay for, or ours."
        case .computers: return "Only needed when Allternit should click and type in apps for you."
        case .phones: return "Talk to Allternit from your phone, and let it use your phone."
        case .tools: return "Command-line agents. They run on a computer and use your plans."
        case .voice: return "Who speaks when Allternit talks back."
        case .apps: return "Mail, files and work apps Allternit can use for you."
        case .channels: return "Where people can message your bots."
        }
    }
}

enum ConnWay: String, Sendable {
    case oauth, code, paste, scan, install, pair, included
}

enum ConnRunsOn: String, Sendable {
    case server, runner, device, computer

    var copy: String {
        switch self {
        case .server: return "Works on any device"
        case .runner: return "Runs on Allternit · no computer needed"
        case .device: return "Saved on this device"
        case .computer: return "Needs your computer"
        }
    }
}

/// How the row's logo is drawn: an asset image, or an SF Symbol on a neutral tile.
enum ConnLogo: Sendable, Equatable {
    case asset(String)
    case symbol(String)
}

struct ConnEntry: Identifiable, Sendable, Equatable {
    let id: String
    let group: ConnGroup
    let name: String
    let logo: ConnLogo
    var blurb: String? = nil
    let ways: [ConnWay]
    var runsOn: ConnRunsOn = .server
    var isNew = false
    /// For keys: the vendor page where the person makes a key.
    var keyPage: String? = nil

    var mainWay: ConnWay { ways.first ?? .included }
}

/// One API key provider, offered inside "An API key".
struct ConnKeyProvider: Identifiable, Sendable, Equatable {
    let id: String
    let name: String
    let logo: ConnLogo
    let keyPage: String
}

enum ConnectionsCatalog {
    static let entries: [ConnEntry] = [
        // Brains
        ConnEntry(id: "allternit", group: .brains, name: "Allternit Cloud", logo: .asset("AProtocol"), ways: [.included]),
        ConnEntry(id: "chatgpt", group: .brains, name: "ChatGPT", logo: .asset("SetupLogoChatGPT"), blurb: "Plus or Pro plan", ways: [.code], runsOn: .computer),
        ConnEntry(id: "claude", group: .brains, name: "Claude", logo: .asset("SetupLogoClaude"), blurb: "Pro or Max plan", ways: [.code], runsOn: .runner),
        ConnEntry(id: "gemini", group: .brains, name: "Gemini", logo: .asset("applogo-googlegemini"), blurb: "Free Google AI Studio key", ways: [.paste], isNew: true,
                  keyPage: "https://aistudio.google.com/apikey"),
        ConnEntry(id: "copilot", group: .brains, name: "GitHub Copilot", logo: .asset("LogoGitHub"), blurb: "Copilot Pro or Business", ways: [.oauth], isNew: true),
        ConnEntry(id: "grok", group: .brains, name: "Grok", logo: .asset("applogo-grok"), blurb: "SuperGrok plan", ways: [.code], runsOn: .runner, isNew: true),
        ConnEntry(id: "kimi", group: .brains, name: "Kimi", logo: .asset("SetupLogoKimi"), blurb: "Kimi plan", ways: [.code], runsOn: .runner),
        ConnEntry(id: "api-key", group: .brains, name: "An API key", logo: .symbol("key"), blurb: "OpenAI, Anthropic, Google, xAI, OpenRouter and more", ways: [.paste]),
        ConnEntry(id: "local", group: .brains, name: "Local brain", logo: .asset("applogo-ollama"), blurb: "Runs offline on your own computer", ways: [.install], runsOn: .computer),

        // Computers
        ConnEntry(id: "cloud-computer", group: .computers, name: "Allternit Cloud computer", logo: .symbol("cloud"), ways: [.included]),
        ConnEntry(id: "this-mac", group: .computers, name: "Your Mac or PC", logo: .symbol("laptopcomputer"), blurb: "Lets Allternit use the apps on it", ways: [.install, .pair], runsOn: .computer),
        ConnEntry(id: "server", group: .computers, name: "Your own server", logo: .symbol("server.rack"), blurb: "A Linux machine you own", ways: [.pair], runsOn: .computer),

        // Phones
        ConnEntry(id: "phone", group: .phones, name: "Your phone", logo: .symbol("iphone"), blurb: "iPhone or Android", ways: [.scan], runsOn: .device),
        ConnEntry(id: "phone-control", group: .phones, name: "Let your bots use this iPhone", logo: .symbol("hand.tap"), blurb: "Taps and types in your phone apps for you", ways: [.install], runsOn: .device),

        // Coding tools
        ConnEntry(id: "gizzi", group: .tools, name: "Gizzi Code", logo: .asset("AProtocol"), ways: [.install], runsOn: .computer),
        ConnEntry(id: "claude-code", group: .tools, name: "Claude Code", logo: .asset("applogo-claude"), ways: [.install, .code], runsOn: .computer),
        ConnEntry(id: "codex", group: .tools, name: "Codex", logo: .asset("applogo-codex"), ways: [.install, .code], runsOn: .computer),
        ConnEntry(id: "gemini-cli", group: .tools, name: "Gemini CLI", logo: .asset("applogo-gemini"), ways: [.install, .code], runsOn: .computer),
        ConnEntry(id: "cursor", group: .tools, name: "Cursor Agent", logo: .asset("applogo-cursor"), ways: [.install, .code], runsOn: .computer),
        ConnEntry(id: "grok-cli", group: .tools, name: "Grok CLI", logo: .asset("applogo-grok"), ways: [.install, .code], runsOn: .computer),
        ConnEntry(id: "kimi-cli", group: .tools, name: "Kimi CLI", logo: .asset("applogo-kimi"), ways: [.install, .code], runsOn: .computer),

        // Voice
        ConnEntry(id: "openai-voice", group: .voice, name: "OpenAI", logo: .asset("applogo-openai"), blurb: "Realtime voice", ways: [.paste], runsOn: .device),
        ConnEntry(id: "elevenlabs", group: .voice, name: "ElevenLabs", logo: .asset("applogo-elevenlabs"), blurb: "Studio voices", ways: [.paste], runsOn: .device),

        // Apps
        ConnEntry(id: "gmail", group: .apps, name: "Gmail", logo: .asset("applogo-gmail"), ways: [.oauth]),
        ConnEntry(id: "drive", group: .apps, name: "Google Drive", logo: .asset("applogo-googledrive"), ways: [.oauth]),
        ConnEntry(id: "slack", group: .apps, name: "Slack", logo: .asset("applogo-slack"), ways: [.oauth]),
        ConnEntry(id: "github", group: .apps, name: "GitHub", logo: .asset("LogoGitHub"), ways: [.oauth]),
        ConnEntry(id: "notion", group: .apps, name: "Notion", logo: .asset("applogo-notion"), ways: [.oauth]),

        // Channels
        ConnEntry(id: "telegram", group: .channels, name: "Telegram", logo: .asset("applogo-telegram"), ways: [.oauth]),
        ConnEntry(id: "whatsapp", group: .channels, name: "WhatsApp", logo: .asset("applogo-whatsapp"), ways: [.oauth]),
        ConnEntry(id: "discord", group: .channels, name: "Discord", logo: .asset("applogo-discord"), ways: [.oauth]),
    ]

    /// Key providers behind "An API key" (cloud-api `user_inference_keys`).
    static let keyProviders: [ConnKeyProvider] = [
        ConnKeyProvider(id: "openai-key", name: "OpenAI", logo: .asset("applogo-openai"), keyPage: "https://platform.openai.com/api-keys"),
        ConnKeyProvider(id: "anthropic-key", name: "Anthropic", logo: .asset("applogo-anthropic"), keyPage: "https://console.anthropic.com/settings/keys"),
        ConnKeyProvider(id: "google-key", name: "Google AI Studio", logo: .asset("applogo-googlegemini"), keyPage: "https://aistudio.google.com/apikey"),
        ConnKeyProvider(id: "xai-key", name: "xAI", logo: .asset("applogo-xai"), keyPage: "https://console.x.ai"),
        ConnKeyProvider(id: "openrouter-key", name: "OpenRouter", logo: .asset("applogo-openrouter"), keyPage: "https://openrouter.ai/settings/keys"),
        ConnKeyProvider(id: "groq-key", name: "Groq", logo: .asset("applogo-groq"), keyPage: "https://console.groq.com/keys"),
        ConnKeyProvider(id: "deepseek-key", name: "DeepSeek", logo: .asset("applogo-deepseek"), keyPage: "https://platform.deepseek.com/api_keys"),
        ConnKeyProvider(id: "kimi-key", name: "Kimi (Moonshot)", logo: .asset("applogo-moonshot"), keyPage: "https://platform.moonshot.ai/console/api-keys"),
        ConnKeyProvider(id: "together-key", name: "Together", logo: .asset("applogo-together"), keyPage: "https://api.together.ai/settings/api-keys"),
        ConnKeyProvider(id: "fireworks-key", name: "Fireworks", logo: .symbol("key"), keyPage: "https://fireworks.ai/account/api-keys"),
        ConnKeyProvider(id: "deepinfra-key", name: "DeepInfra", logo: .symbol("key"), keyPage: "https://deepinfra.com/dash/api_keys"),
    ]

    static func entry(_ id: String) -> ConnEntry? { entries.first { $0.id == id } }
    static func keyProvider(_ id: String) -> ConnKeyProvider? { keyProviders.first { $0.id == id } }

    /// Every id the app knows (rows plus key providers); the server's other ids are ignored.
    static let knownIds: Set<String> = Set(entries.map(\.id)).union(keyProviders.map(\.id))

    /// The coding tools whose sign-in runs on a computer (contract §4, runtime CLI logins).
    static let cliLoginIds: Set<String> = ["claude-code", "codex", "gemini-cli", "cursor", "grok-cli", "kimi-cli"]

    /// What a row shows when the server hasn't answered for it (contract §3:
    /// missing ids fall back to the catalog with `state: off`). Rows that can
    /// never work without the server's say-so start unavailable, so a stale
    /// or offline app never shows a Connect button that fails.
    static func fallbackRow(_ id: String, computerOnline: Bool) -> ConnectionWire {
        switch id {
        case "claude":
            return ConnectionWire(id: id, reasonCode: "not_available_yet", available: false,
                                  unavailableReason: "Not available yet. Use a key for now.", fallbacks: ["anthropic-key"])
        case "grok":
            return ConnectionWire(id: id, reasonCode: "not_available_yet", available: false,
                                  unavailableReason: "Not available yet. Use a key for now.", fallbacks: ["xai-key"])
        case "kimi":
            return ConnectionWire(id: id, reasonCode: "not_available_yet", available: false,
                                  unavailableReason: "Not available yet. Use a key for now.", fallbacks: ["kimi-key"])
        case "copilot":
            return ConnectionWire(id: id, available: false, unavailableReason: "Coming soon.", fallbacks: ["github"])
        case "chatgpt" where !computerOnline:
            return ConnectionWire(id: id, reasonCode: "no_computer", available: false,
                                  unavailableReason: "Needs your own computer for now. Or use an OpenAI key.", fallbacks: ["this-mac", "openai-key"])
        case "local", "gizzi", "claude-code", "codex", "gemini-cli", "cursor", "grok-cli", "kimi-cli":
            return computerOnline
                ? ConnectionWire(id: id)
                : ConnectionWire(id: id, reasonCode: "no_computer", available: false, unavailableReason: "Runs on a computer.", fallbacks: ["this-mac"])
        default:
            return ConnectionWire(id: id)
        }
    }

    /// Contract §5: the plain text for each reason code.
    static func reasonText(_ code: String?, name: String) -> String? {
        switch code {
        case "login_expired": return "Sign-in expired"
        case "key_rejected": return "The key was rejected"
        case "vendor_limited": return "\(name) limited this plan"
        case "no_computer": return "Needs your own computer for now"
        case "not_available_yet": return "Not available yet"
        case "computer_offline": return "Your computer is offline"
        case "could_not_start": return "Could not start. Try again"
        default: return nil
        }
    }

    /// The button text for a fallback id.
    static func fallbackLabel(_ id: String) -> String {
        if id == "this-mac" { return "Add a computer" }
        if id == "cloud-computer" { return "See plans" }
        if id == "api-key" || id.hasSuffix("-key") { return "Use a key" }
        if let e = entry(id) { return "Use \(e.name)" }
        return "Other way"
    }
}
