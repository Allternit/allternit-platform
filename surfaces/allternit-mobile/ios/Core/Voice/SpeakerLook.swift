import VoiceRingKit
import SwiftUI

/// Live mode's speaker look (allternit-ai src/lib/voice/speaker-style.ts):
/// what moves around a bot's avatar in voice, its colour, and caption size.
/// Settings → Voice holds the default; each bot can keep its own ("Same as
/// Settings" follows the default). Kept on this device, like the web.
///
/// iPhone draws every style the web does: the Ring (voice-ring's SwiftUI
/// port), Orbit, Sash, Signal, Swell and Pixel (ported into the vendored kit,
/// OrbTypes.swift), and our own Blocks, ASCII, Glow and Still.
enum SpeakerStyle: String, CaseIterable, Identifiable, Sendable {
    case ring, orbit, sash, signal, swell, pixel, blocks, ascii, glow, still
    var id: String { rawValue }

    var name: String {
        switch self {
        case .ring: return "Ring"
        case .orbit: return "Orbit"
        case .sash: return "Sash"
        case .signal: return "Signal"
        case .swell: return "Swell"
        case .pixel: return "Pixel"
        case .blocks: return "Blocks"
        case .ascii: return "ASCII"
        case .glow: return "Glow"
        case .still: return "Still"
        }
    }

    var note: String {
        switch self {
        case .ring: return "A ring of light that bends with the voice"
        case .blocks: return "A-block squares pulse around the face"
        case .ascii: return "A ring of characters, like the ASCII backgrounds"
        case .glow: return "Only a soft halo in the colour"
        case .still: return "No effect; the avatar lifts a little while it talks"
        case .orbit: return "Particles on tilted orbits; a voice pulls them in"
        case .sash: return "A band of dotted lanes; a voice deepens the wave"
        case .signal: return "A wireframe sphere that a voice bends like a skin"
        case .swell: return "A halftone sphere of drifting clouds"
        case .pixel: return "A ball of square pixels, lit in rings"
        }
    }

    /// The Motion set (voice-ring) or ours (Allternit), as the web groups them.
    var isMotion: Bool { [.ring, .orbit, .sash, .signal, .swell, .pixel].contains(self) }
    /// The ink orb this style draws, if it is one.
    var orbType: OrbType? { OrbType(rawValue: rawValue) }
}

enum RingPalette: String, CaseIterable, Identifiable, Sendable {
    case spectrum, bot, ember, ocean, aurora, rose, gold, graphite
    var id: String { rawValue }

    var name: String {
        switch self {
        case .spectrum: return "Spectrum"
        case .bot: return "Bot's colour"
        case .ember: return "Ember"
        case .ocean: return "Ocean"
        case .aurora: return "Aurora"
        case .rose: return "Rose"
        case .gold: return "Gold"
        case .graphite: return "Graphite"
        }
    }

    /// The palette's colours, or nil for the ring's own spectrum.
    func colors(bot: Color?) -> [Color]? {
        switch self {
        case .spectrum: return nil
        case .bot:
            // A bot without a colour yet keeps the ring's own spectrum.
            guard let c = bot else { return nil }
            return [c, c.opacity(0.75), c]
        case .ember: return [Color(hex: "#FF6A3D"), Color(hex: "#FFB347"), Color(hex: "#D9433B")]
        case .ocean: return [Color(hex: "#2F80ED"), Color(hex: "#56CCF2"), Color(hex: "#1B4DB1")]
        case .aurora: return [Color(hex: "#34D399"), Color(hex: "#22D3EE"), Color(hex: "#A78BFA")]
        case .rose: return [Color(hex: "#F472B6"), Color(hex: "#FB7185"), Color(hex: "#C084FC")]
        case .gold: return [Color(hex: "#D4A93C"), Color(hex: "#F5DFA0"), Color(hex: "#9C7A22")]
        case .graphite: return [Color(hex: "#3A3B3F"), Color(hex: "#8C8E94"), Color(hex: "#1E1F22")]
        }
    }

    static let spectrumColors: [Color] = ["#ff5f6d", "#ffc371", "#47e891", "#3fa9f5", "#a86bff", "#ff5f6d"].map { Color(hex: $0) }
    /// Muted and stopped calls.
    static let idle: [Color] = [Color(hex: "#9a9a95"), Color(hex: "#c9c9c4"), Color(hex: "#7c7c78")]
}

enum CaptionSize: String, CaseIterable, Identifiable, Sendable {
    case small = "S", medium = "M", large = "L"
    var id: String { rawValue }
    var name: String { self == .small ? "Small" : self == .medium ? "Medium" : "Large" }
    var points: CGFloat { self == .small ? 15 : self == .medium ? 17 : 21 }
}

/// The defaults and each bot's own picks.
@MainActor
final class SpeakerLookStore: ObservableObject {
    static let shared = SpeakerLookStore()

    private let defaults: UserDefaults
    private static let styleKey = "voice.speakerStyle"
    private static let paletteKey = "voice.ringPalette"
    private static let captionKey = "voice.captionSize"

    @Published var style: SpeakerStyle { didSet { defaults.set(style.rawValue, forKey: Self.styleKey) } }
    @Published var palette: RingPalette { didSet { defaults.set(palette.rawValue, forKey: Self.paletteKey) } }
    @Published var captionSize: CaptionSize { didSet { defaults.set(captionSize.rawValue, forKey: Self.captionKey) } }
    /// Bumps when a bot's own pick changes, so views re-read it.
    @Published private(set) var botRevision = 0

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        style = SpeakerStyle(rawValue: defaults.string(forKey: Self.styleKey) ?? "") ?? .ring
        palette = RingPalette(rawValue: defaults.string(forKey: Self.paletteKey) ?? "") ?? .bot
        captionSize = CaptionSize(rawValue: defaults.string(forKey: Self.captionKey) ?? "") ?? .medium
    }

    private static func botKey(_ base: String, _ botId: String) -> String { "\(base).for.bot:\(botId)" }

    /// The bot's own style, or nil while it follows Settings.
    func ownStyle(for botId: String) -> SpeakerStyle? {
        SpeakerStyle(rawValue: defaults.string(forKey: Self.botKey(Self.styleKey, botId)) ?? "")
    }

    func ownPalette(for botId: String) -> RingPalette? {
        RingPalette(rawValue: defaults.string(forKey: Self.botKey(Self.paletteKey, botId)) ?? "")
    }

    func setOwnStyle(_ value: SpeakerStyle?, for botId: String) {
        defaults.set(value?.rawValue, forKey: Self.botKey(Self.styleKey, botId))
        botRevision += 1
    }

    func setOwnPalette(_ value: RingPalette?, for botId: String) {
        defaults.set(value?.rawValue, forKey: Self.botKey(Self.paletteKey, botId))
        botRevision += 1
    }

    /// What live mode uses for this bot: its own pick, else Settings.
    func look(for botId: String?) -> (style: SpeakerStyle, palette: RingPalette) {
        guard let botId else { return (style, palette) }
        return (ownStyle(for: botId) ?? style, ownPalette(for: botId) ?? palette)
    }
}
