import SwiftUI
import UIKit

/// A bot's avatar, resolved the way the web's `BotAvatar` does
/// (allternit-ai src/views/bots/BotAvatar.tsx + lib/agents/bot-avatar-config.ts):
/// `config.avatar` (what Edit bot writes) → the `avatar` column → fallback.
/// Both may be a JSON object or a JSON string of one.
///
/// - `pet.spriteUrl`: a Codex-style sprite sheet (192×208 frames, 8×9);
///   the first row plays as the "living" idle loop (static under Reduce Motion).
/// - `type: "image"` with `uri`, or `data.url`: an uploaded / photo avatar.
/// - anything else: the existing `AgentAvatarView` (emoji / grid identicon),
///   or the identicon when the avatar is an object we don't draw natively.
struct BotAvatarView: View {
    let bot: AgentRecord
    var size: CGFloat = 40
    /// Animates sprite avatars (the web's `live` prop).
    var live: Bool = false
    /// Working: the A:// orb runs its working choreography.
    var working: Bool = false
    /// The web's framed tile (rounded square, hairline, card fill) for the A:// orb.
    var framed: Bool = false

    var body: some View {
        if bot.isPrimaryTemplateBot {
            // web PrimaryBotOrb: the A:// main bot is the Allternit orb, 62% of the box.
            AllternitOrbView(state: working ? .working : .idle, size: (size * 0.62).rounded(), label: bot.botDisplayName)
                .frame(width: size, height: size)
                .background {
                    if framed {
                        RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel"))
                            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                    }
                }
        } else {
            resolved
        }
    }

    @ViewBuilder private var resolved: some View {
        switch BotAvatarSource.resolve(bot) {
        case .sprite(let url):
            SpriteAvatar(url: url, size: size, live: live)
                .frame(width: size, height: size)
                .accessibilityLabel("\(bot.botDisplayName) avatar")
        case .image(let url):
            AsyncImage(url: url) { phase in
                if let image = phase.image {
                    image.resizable().scaledToFill()
                } else {
                    AgentIdenticonView(seed: bot.id, size: size * 0.62)
                }
            }
            .frame(width: size, height: size)
            .clipShape(RoundedRectangle(cornerRadius: size * 0.3, style: .continuous))
            .accessibilityLabel("\(bot.botDisplayName) avatar")
        case .identicon:
            ZStack {
                RoundedRectangle(cornerRadius: size * 0.325, style: .continuous)
                    .fill(Color("BgSecondary"))
                AgentIdenticonView(seed: bot.id, size: size * 0.62)
            }
            .frame(width: size, height: size)
        case .legacy:
            AgentAvatarView(agent: bot, size: size)
        }
    }
}

/// Avatar with the rail's status dot: green while working, amber while it
/// needs you (BotRailRows working-dot / waiting-dot).
struct BotStatusAvatar: View {
    let bot: AgentRecord
    var size: CGFloat = 40
    var working = false
    var waiting = false
    var live = false

    var body: some View {
        BotAvatarView(bot: bot, size: size, live: live, working: working)
            .overlay(alignment: .bottomTrailing) {
                if waiting || working {
                    Circle()
                        .fill(waiting ? Theme.statusWarning : Theme.statusSuccess)
                        .frame(width: max(9, size * 0.26), height: max(9, size * 0.26))
                        .overlay(Circle().stroke(Color("BgPrimary"), lineWidth: 2))
                        .accessibilityLabel(waiting ? "Needs you" : "Working")
                }
            }
    }
}

extension AgentRecord {
    /// The A:// main bot: made from the main bot template (web isPrimaryTemplateBot),
    /// or a bot still named A://.
    var isPrimaryTemplateBot: Bool {
        if case .string(let id)? = config?["templateId"], id == "primary-a" { return true }
        if case .string(let id)? = config?["template_id"], id == "primary-a" { return true }
        return botDisplayName == "A://"
    }
}

enum BotAvatarSource: Equatable {
    case sprite(URL)
    case image(URL)
    case identicon
    case legacy

    static func resolve(_ bot: AgentRecord) -> BotAvatarSource {
        let candidates: [JSONValue?] = [
            bot.config?["avatar"],
            bot.avatar.map { .string($0) },
        ]
        for candidate in candidates {
            guard let object = object(candidate) else { continue }
            if case .object(let pet)? = object["pet"],
               case .string(let sprite)? = pet["spriteUrl"], let url = webAssetURL(sprite) {
                return .sprite(url)
            }
            if case .string(let type)? = object["type"], type == "image" {
                if case .string(let uri)? = object["uri"], let url = webAssetURL(uri) { return .image(url) }
                if case .object(let data)? = object["data"],
                   case .string(let uri)? = data["url"], let url = webAssetURL(uri) { return .image(url) }
            }
            // An avatar object we don't draw natively (mascot, geometric,
            // pet vector): never show its JSON as text.
            return .identicon
        }
        if let avatar = bot.avatar, avatar.hasPrefix("http"), let url = URL(string: avatar) {
            return .image(url)
        }
        return .legacy
    }

    /// Pack sprites are stored as web paths ("/avatar-packs/…"), served by
    /// the web app; a bare path would never load on the phone.
    static func webAssetURL(_ raw: String) -> URL? {
        if raw.hasPrefix("/") { return URL(string: raw, relativeTo: webOrigin)?.absoluteURL }
        return URL(string: raw)
    }

    static let webOrigin = URL(string: "https://ai.allternit.com")!

    private static func object(_ value: JSONValue?) -> [String: JSONValue]? {
        switch value {
        case .object(let dict)?:
            return dict
        case .string(let text)?:
            let trimmed = text.trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix("{"),
                  let parsed = try? JSONDecoder().decode(JSONValue.self, from: Data(trimmed.utf8)),
                  case .object(let dict) = parsed else { return nil }
            return dict
        default:
            return nil
        }
    }
}

/// CGImage crosses the detached decode task; it is immutable.
private struct SpriteFrames: @unchecked Sendable {
    let images: [CGImage]
}

/// Frames of a sprite sheet, cached per URL.
@MainActor
private final class SpriteFrameCache {
    static let shared = SpriteFrameCache()
    private var frames: [URL: [CGImage]] = [:]
    private var loading: [URL: Task<SpriteFrames, Never>] = [:]

    func frames(for url: URL) async -> [CGImage] {
        if let cached = frames[url] { return cached }
        if let running = loading[url] { return await running.value.images }
        let task = Task.detached(priority: .utility) { () -> SpriteFrames in
            guard let result = try? await URLSession.shared.data(from: url),
                  let image = UIImage(data: result.0)?.cgImage else { return SpriteFrames(images: []) }
            // 8 columns × 9 rows; row 0 is the idle loop.
            let columns = 8, rows = 9
            let width = image.width / columns, height = image.height / rows
            guard width > 0, height > 0 else { return SpriteFrames(images: []) }
            return SpriteFrames(images: (0..<columns).compactMap { column in
                image.cropping(to: CGRect(x: column * width, y: 0, width: width, height: height))
            })
        }
        loading[url] = task
        let result = await task.value.images
        loading[url] = nil
        frames[url] = result
        return result
    }
}

/// A sprite-sheet avatar (row 0 is the idle loop). Also used by the Meet
/// your bots flow for template looks.
struct SpriteAvatar: View {
    let url: URL
    let size: CGFloat
    let live: Bool
    @State private var frames: [CGImage] = []
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    init(url: URL, size: CGFloat, live: Bool) {
        self.url = url
        self.size = size
        self.live = live
    }

    var body: some View {
        Group {
            if frames.isEmpty {
                Color.clear
            } else if live && !reduceMotion && frames.count > 1 {
                TimelineView(.periodic(from: .now, by: 0.12)) { context in
                    let index = Int(context.date.timeIntervalSinceReferenceDate / 0.12) % frames.count
                    frame(frames[index])
                }
            } else {
                frame(frames[0])
            }
        }
        .task(id: url) { frames = await SpriteFrameCache.shared.frames(for: url) }
    }

    private func frame(_ image: CGImage) -> some View {
        Image(decorative: image, scale: 1)
            .resizable()
            .interpolation(.none)
            .scaledToFit()
            .frame(width: size, height: size)
    }
}
