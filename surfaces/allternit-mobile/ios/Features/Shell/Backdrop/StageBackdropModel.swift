import SwiftUI

/// The bot-screen backgrounds (allternit-ai stage-backdrop-model.ts). Two
/// sets share one preference: Classic (thirteen soft looks drawn in the
/// screen's colours) and ASCII (twenty looks drawn in characters).
/// Settings → Appearance picks the default behind bot chats, group chats and,
/// when switched on, Code mode. A look picked on a bot, a group chat or a Code
/// workspace applies there only (its scope); Settings never overrides it.
struct BackdropOption: Identifiable, Hashable {
    let id: String
    let name: String
    let note: String
}

enum BackdropFamily: String, CaseIterable, Identifiable {
    case ascii, classic
    var id: String { rawValue }
    var label: String { self == .ascii ? "ASCII" : "Classic" }
}

/// Where a background can show. Code mode is off until the person turns it on.
enum BackdropSurface: String, CaseIterable, Identifiable {
    case bot, group, project, code
    var id: String { rawValue }

    var label: String {
        switch self {
        case .bot: return "Bot chats"
        case .group: return "Group chats"
        case .project: return "Bot project homes"
        case .code: return "Code mode"
        }
    }

    var defaultOn: Bool { self != .code }
}

enum BackdropMotion: String, CaseIterable { case still, slow, normal }
enum AsciiCharSize: String, CaseIterable { case small, medium, large }

enum StageBackdrops {
    static let classic: [BackdropOption] = [
        BackdropOption(id: "aurora", name: "Aurora", note: "Soft colour clouds drift behind frosted glass"),
        BackdropOption(id: "mesh", name: "Glass mesh", note: "A slow mesh gradient with fine grain"),
        BackdropOption(id: "constellation", name: "Constellation", note: "Drifting points that link up, like the bot graph"),
        BackdropOption(id: "spotlight", name: "Spotlight", note: "A soft light follows whoever is speaking"),
        BackdropOption(id: "horizon", name: "Horizon", note: "Slow tinted waves under a glass composer"),
        BackdropOption(id: "silk", name: "Silk", note: "Wide ribbons of colour sway slowly across the screen"),
        BackdropOption(id: "bokeh", name: "Bokeh", note: "Soft out-of-focus lights float upward"),
        BackdropOption(id: "grid", name: "Grid glow", note: "A faint dot grid with a slow pulse of light passing through"),
        BackdropOption(id: "lava", name: "Lava", note: "Blobs merge and split, like a lava lamp"),
        BackdropOption(id: "prism", name: "Prism", note: "A conic gradient turns very slowly, like light through glass"),
        BackdropOption(id: "fireflies", name: "Fireflies", note: "Small points drift and blink, warm in the dark"),
        BackdropOption(id: "dusk", name: "Dusk", note: "A sky gradient that shifts slowly, your colour glowing low"),
        BackdropOption(id: "plain", name: "Plain", note: "Today's flat colour, for those who want it still"),
    ]

    static let ascii: [BackdropOption] = AsciiScenes.all.map { BackdropOption(id: $0.id, name: $0.name, note: $0.note) }

    static func list(_ family: BackdropFamily) -> [BackdropOption] { family == .ascii ? ascii : classic }

    static let defaultVariant = "aurora"

    private static let ids = Set((classic + ascii).map(\.id))
    static func isVariant(_ value: String?) -> Bool { value.map { ids.contains($0) } ?? false }
    static func isAscii(_ id: String) -> Bool { id.hasPrefix("ascii-") }
    static func family(_ id: String) -> BackdropFamily { isAscii(id) ? .ascii : .classic }
    static func name(_ id: String) -> String { (classic + ascii).first { $0.id == id }?.name ?? "Plain" }

    /// Sample colours for previews (Settings tiles, the quick switch).
    static let sampleColors = ["#F2A93B", "#0F766E", "#60A5FA", "#A78BFA", "#E07A5F"]

    /// The colours a backdrop draws with: one colour gets a lighter partner so
    /// the clouds still read as two tones; several (a group) are used as given.
    static func palette(_ colors: [String?]) -> [Color] {
        let list = colors.compactMap { $0 }
        if list.isEmpty {
            let accent = Color("AccentPrimary")
            return [accent, accent.opacity(0.7), accent]
        }
        if list.count == 1 {
            var light = AsciiHSL(hex: list[0])
            light.l += (1 - light.l) * 0.3
            let c = Color(hex: list[0])
            return [c, light.color, c]
        }
        return list.map { Color(hex: $0) }
    }
}

/// A screen that keeps its own look: one bot, one group chat or one Code
/// workspace (`bot:<id>`, `group:<id>`, `code:<workspaceId>`).
struct BackdropScope: Hashable {
    let raw: String
    static func bot(_ id: String) -> BackdropScope { BackdropScope(raw: "bot:\(id)") }
    static func group(_ id: String) -> BackdropScope { BackdropScope(raw: "group:\(id)") }
    /// Code mode keeps one look per workspace.
    static func code(_ workspaceId: String?) -> BackdropScope {
        BackdropScope(raw: "code:\((workspaceId?.isEmpty == false ? workspaceId : nil) ?? "default")")
    }
}

/// The background preferences, kept like the web's appearance settings
/// (`appearance.stageBackground`, `appearance.background.*`,
/// `appearance.stageBackground.for.<scope>`), live across every reader.
@MainActor
final class StageBackdropStore: ObservableObject {
    static let shared = StageBackdropStore()

    private let defaults = UserDefaults.standard
    private static let prefix = "allternit.settings.v1."
    static let variantKey = "appearance.stageBackground"
    static func surfaceKey(_ s: BackdropSurface) -> String { "appearance.background.on.\(s.rawValue)" }
    static func scopeKey(_ scope: BackdropScope) -> String { "appearance.stageBackground.for.\(scope.raw)" }

    @Published var variant: String { didSet { save(Self.variantKey, variant) } }
    @Published var motion: BackdropMotion { didSet { save("appearance.background.motion", motion.rawValue) } }
    @Published var colour: AsciiColourMode { didSet { save("appearance.background.colour", colour.rawValue) } }
    @Published var chars: AsciiCharMode { didSet { save("appearance.background.chars", chars.rawValue) } }
    @Published var charSize: AsciiCharSize { didSet { save("appearance.background.charSize", charSize.rawValue) } }
    /// How much is drawn, 0.4–1.6.
    @Published var density: Double { didSet { save("appearance.background.density", density) } }
    /// The fade behind the conversation, 20–70 (%). Never below 20, so text always has a backing.
    @Published var veil: Double { didSet { save("appearance.background.veil", veil) } }
    @Published private var surfaces: [BackdropSurface: Bool] = [:]
    @Published private var scoped: [String: String] = [:]

    private init() {
        func read(_ key: String) -> Any? { UserDefaults.standard.object(forKey: Self.prefix + key) }
        if let stored = read(Self.variantKey) as? String, StageBackdrops.isVariant(stored) {
            variant = stored
        } else {
            variant = StageBackdrops.defaultVariant
        }
        motion = (read("appearance.background.motion") as? String).flatMap(BackdropMotion.init(rawValue:)) ?? .normal
        colour = (read("appearance.background.colour") as? String).flatMap(AsciiColourMode.init(rawValue:)) ?? .scene
        chars = (read("appearance.background.chars") as? String).flatMap(AsciiCharMode.init(rawValue:)) ?? .design
        charSize = (read("appearance.background.charSize") as? String).flatMap(AsciiCharSize.init(rawValue:)) ?? .medium
        density = min(1.6, max(0.4, read("appearance.background.density") as? Double ?? 1))
        veil = min(70, max(20, read("appearance.background.veil") as? Double ?? 35))
        for s in BackdropSurface.allCases {
            surfaces[s] = read(Self.surfaceKey(s)) as? Bool ?? s.defaultOn
        }
        #if DEBUG
        // `-stage-background <id>`: that look everywhere, Code mode included (screenshots).
        if let forced = UserDefaults.standard.string(forKey: "stage-background"), StageBackdrops.isVariant(forced) {
            variant = forced
            for s in BackdropSurface.allCases { surfaces[s] = true }
        }
        #endif
    }

    private func save(_ key: String, _ value: Any?) {
        if let value { defaults.set(value, forKey: Self.prefix + key) } else { defaults.removeObject(forKey: Self.prefix + key) }
    }

    func isOn(_ surface: BackdropSurface) -> Bool { surfaces[surface] ?? surface.defaultOn }

    func setOn(_ surface: BackdropSurface, _ on: Bool) {
        surfaces[surface] = on
        save(Self.surfaceKey(surface), on)
    }

    /// This screen's own look, or nil while it follows Settings.
    func own(_ scope: BackdropScope?) -> String? {
        guard let scope else { return nil }
        if let cached = scoped[scope.raw] { return cached.isEmpty ? nil : cached }
        let stored = defaults.string(forKey: Self.prefix + Self.scopeKey(scope))
        return StageBackdrops.isVariant(stored) ? stored : nil
    }

    /// nil goes back to "Same as Settings".
    func setOwn(_ scope: BackdropScope, _ value: String?) {
        scoped[scope.raw] = value ?? ""
        save(Self.scopeKey(scope), value)
    }

    /// The background a surface shows: the screen's own look when it has one,
    /// else the Settings default, or Plain when that surface is off.
    func variant(for surface: BackdropSurface, scope: BackdropScope?) -> String {
        if let own = own(scope) { return own }
        return isOn(surface) ? variant : "plain"
    }

    var fontScale: Double {
        switch charSize {
        case .small: return 0.85
        case .medium: return 1
        case .large: return 1.25
        }
    }
}

/// The open screen's background carried into the drawer (RailBackdrop.tsx):
/// a bot chat, a group room or Code publishes its colours and look while it
/// shows; the drawer draws them behind frosted glass, so the rail and the
/// screen read as one.
@MainActor
final class RailBackdropStore: ObservableObject {
    static let shared = RailBackdropStore()

    struct Look: Equatable {
        let colors: [String]
        let variant: String
        let mode: ShellMode
    }

    @Published private(set) var look: Look?
    private var owner: UUID?

    func publish(_ token: UUID, _ look: Look) {
        owner = token
        if self.look != look { self.look = look }
    }

    func clear(_ token: UUID) {
        guard owner == token else { return }
        owner = nil
        look = nil
    }
}

private struct PublishRailBackdrop: ViewModifier {
    let look: RailBackdropStore.Look?
    @State private var token = UUID()

    func body(content: Content) -> some View {
        content
            .onAppear { if let look { RailBackdropStore.shared.publish(token, look) } }
            .onChange(of: look) { _, next in
                if let next { RailBackdropStore.shared.publish(token, next) } else { RailBackdropStore.shared.clear(token) }
            }
            .onDisappear { RailBackdropStore.shared.clear(token) }
    }
}

extension View {
    /// Shares this screen's background with the drawer while it shows (nil: none).
    func publishesRailBackdrop(_ look: RailBackdropStore.Look?) -> some View {
        modifier(PublishRailBackdrop(look: look))
    }
}

// MARK: - Environment

private struct BackdropUnderneathKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    /// A screen behind this one already draws a background: page fills
    /// (BotWashedBackground, the terminal colour) step aside so it shows.
    var backdropUnderneath: Bool {
        get { self[BackdropUnderneathKey.self] }
        set { self[BackdropUnderneathKey.self] = newValue }
    }
}
