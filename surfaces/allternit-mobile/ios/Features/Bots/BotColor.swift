import SwiftUI
import UIKit

/// A bot's colour, read from its own art (allternit-ai
/// src/lib/bots/bot-art-color.ts + bot-profile.ts getBotOwnColor).
///
/// Bots carry the workspace default accent unless someone picked one, so a
/// bot's real colour (Gem is pink, Ini is green) only lives in its avatar.
/// The store samples the avatar once and keeps the result per bot; the page
/// wash, the Bots rail and your message bubbles then wear that colour.
///
/// It also tracks the bot that is "on screen" (picked in To: on the Bots
/// home, or whose chat is open) so the drawer and the screens agree.
@MainActor
final class BotColorStore: ObservableObject {
    static let shared = BotColorStore()

    private static let key = "allternit.botArtColor.v1"
    /// The workspace default accents: a bot with one of these has no colour of its own.
    private static let defaultAccents: Set<String> = ["#b08d6e", "#d4b08c"]

    @Published private(set) var sampled: [String: String]
    /// Screens that show a bot, newest last; the last one is the active bot.
    @Published private var claims: [(token: UUID, botId: String)] = []
    private var sampling: Set<String> = []

    private init() {
        sampled = (UserDefaults.standard.dictionary(forKey: Self.key) as? [String: String]) ?? [:]
    }

    /// The bot's colour as hex: one it was given, else the one read from its
    /// art, else nil (greyscale art, pixel marks: the neutral page stays).
    func hex(for bot: AgentRecord?) -> String? {
        guard let bot else { return nil }
        if let own = bot.botAccentColor?.trimmingCharacters(in: .whitespaces), !own.isEmpty,
           !Self.defaultAccents.contains(own.lowercased()) {
            return own
        }
        if let color = sampled[bot.id] { return color }
        sample(bot)
        return nil
    }

    /// The colour of the bot on screen, if it has one.
    var activeHex: String? {
        guard let id = claims.last?.botId else { return nil }
        return hex(for: BotModeStore.shared.bot(id))
    }

    func claim(_ token: UUID, botId: String?) {
        claims.removeAll { $0.token == token }
        if let botId { claims.append((token, botId)) }
    }

    func release(_ token: UUID) {
        claims.removeAll { $0.token == token }
    }

    private func sample(_ bot: AgentRecord) {
        guard !sampling.contains(bot.id) else { return }
        let url: URL
        let sprite: Bool
        switch BotAvatarSource.resolve(bot) {
        case .sprite(let found): url = found; sprite = true
        case .image(let found): url = found; sprite = false
        default: return
        }
        sampling.insert(bot.id)
        let id = bot.id
        Task {
            let color = await Task.detached(priority: .utility) {
                await BotColorStore.artColor(url: url, sprite: sprite)
            }.value
            guard let color else { return }
            sampled[id] = color
            UserDefaults.standard.set(sampled, forKey: Self.key)
        }
    }

    /// Draws the art (a sprite's first frame) at 48×48 and reads its colour.
    nonisolated private static func artColor(url: URL, sprite: Bool) async -> String? {
        guard let (data, _) = try? await URLSession.shared.data(from: url),
              var image = UIImage(data: data)?.cgImage else { return nil }
        if sprite {
            let width = image.width / 8, height = image.height / 9
            guard width > 0, height > 0,
                  let frame = image.cropping(to: CGRect(x: 0, y: 0, width: width, height: height)) else { return nil }
            image = frame
        }
        let size = 48
        var pixels = [UInt8](repeating: 0, count: size * size * 4)
        let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
            guard let context = CGContext(
                data: buffer.baseAddress, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            ) else { return false }
            context.draw(image, in: CGRect(x: 0, y: 0, width: size, height: size))
            return true
        }
        return drawn ? dominantColor(pixels) : nil
    }

    /// The dominant saturated colour of some RGBA pixels, or nil when the art
    /// is greyscale (web dominantColor: 24 hue bins weighted by saturation).
    nonisolated static func dominantColor(_ data: [UInt8]) -> String? {
        var bins = [(w: Double, r: Double, g: Double, b: Double)](repeating: (0, 0, 0, 0), count: 24)
        var i = 0
        while i + 3 < data.count {
            defer { i += 4 }
            guard data[i + 3] >= 200 else { continue }
            let r = Double(data[i]) / 255, g = Double(data[i + 1]) / 255, b = Double(data[i + 2]) / 255
            let maxV = max(r, g, b), minV = min(r, g, b)
            let l = (maxV + minV) / 2
            let d = maxV - minV
            guard d > 0 else { continue }
            let s = d / (1 - abs(2 * l - 1))
            guard s >= 0.3, l >= 0.18, l <= 0.9 else { continue }
            var h: Double
            if maxV == r { h = ((g - b) / d).truncatingRemainder(dividingBy: 6) }
            else if maxV == g { h = (b - r) / d + 2 }
            else { h = (r - g) / d + 4 }
            h = (h * 60 + 360).truncatingRemainder(dividingBy: 360)
            let index = Int(h / 15) % 24
            bins[index].w += s
            bins[index].r += Double(data[i]) * s
            bins[index].g += Double(data[i + 1]) * s
            bins[index].b += Double(data[i + 2]) * s
        }
        guard let best = bins.max(by: { $0.w < $1.w }), best.w >= 8 else { return nil }
        func hex(_ value: Double) -> String { String(format: "%02x", Int(value.rounded())) }
        return "#\(hex(best.r / best.w))\(hex(best.g / best.w))\(hex(best.b / best.w))"
    }
}

// MARK: - Environment

private struct BotTintKey: EnvironmentKey {
    static let defaultValue: String? = nil
}

extension EnvironmentValues {
    /// The colour (hex) of the bot this screen belongs to; nil keeps it neutral.
    var botTint: String? {
        get { self[BotTintKey.self] }
        set { self[BotTintKey.self] = newValue }
    }
}

/// Marks a screen as showing a bot, so the drawer wears its colour.
private struct BotTintClaim: ViewModifier {
    let botId: String?
    @State private var token = UUID()

    func body(content: Content) -> some View {
        content
            .onAppear { BotColorStore.shared.claim(token, botId: botId) }
            .onChange(of: botId) { _, id in BotColorStore.shared.claim(token, botId: id) }
            .onDisappear { BotColorStore.shared.release(token) }
    }
}

extension View {
    func claimsBotTint(_ botId: String?) -> some View {
        modifier(BotTintClaim(botId: botId))
    }
}

// MARK: - Wash

/// The colour wash (BotColorWash.tsx): the top of the page wears a faint
/// vertical wash of the bot's colour (13% over the page, 6% at 45%, gone by
/// the bottom), min(620, 55% of the screen) tall. Fades in 240 ms, out
/// 160 ms, crossfades between bots; instant under Reduce Motion.
struct BotColorWash: View {
    let color: String?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    init(color: String?) {
        self.color = color
    }

    static var height: CGFloat { min(620, UIScreen.main.bounds.height * 0.55) }

    var body: some View {
        ZStack {
            if let color {
                let tint = Color(hex: color)
                LinearGradient(
                    stops: [
                        .init(color: tint.opacity(0.13), location: 0),
                        .init(color: tint.opacity(0.06), location: 0.45),
                        .init(color: tint.opacity(0), location: 1),
                    ],
                    startPoint: .top, endPoint: .bottom
                )
                .id(color)
                .transition(reduceMotion ? .identity : .asymmetric(
                    insertion: .opacity.animation(.timingCurve(0.2, 0.8, 0.2, 1, duration: 0.24)),
                    removal: .opacity.animation(.timingCurve(0.4, 0, 1, 1, duration: 0.16))
                ))
            }
        }
        .frame(height: Self.height)
        .animation(reduceMotion ? nil : .easeOut(duration: 0.24), value: color)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// The page background with the bot's wash, measured from the top of the
/// screen. Put it behind a whole page with `.ignoresSafeArea()`, or behind a
/// top bar with `.ignoresSafeArea(edges: .top)`: both show the same slice of
/// one wash, so the bar and the page meet without a seam.
struct BotWashedBackground: View {
    /// nil reads the screen's `botTint`.
    var color: String??
    @Environment(\.botTint) private var tint

    init(color: String?? = nil) {
        self.color = color
    }

    var body: some View {
        Color("BgPrimary")
            .overlay(alignment: .top) { BotColorWash(color: color ?? tint) }
            .clipped()
    }
}

