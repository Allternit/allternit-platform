import SwiftUI
import UIKit
import SwiftDraw

/// A bot's avatar, resolved the way the web's `BotAvatar` does
/// (allternit-ai src/views/bots/BotAvatar.tsx + lib/agents/bot-avatar-config.ts):
/// `config.avatar` (what Edit bot writes) → the `avatar` column → fallback.
/// Both may be a JSON object or a JSON string of one.
///
/// - `pet.spriteUrl`: a Codex-style sprite sheet (192×208 frames, 8×9);
///   the first row plays as the "living" idle loop (static under Reduce Motion).
/// - `type: "image"` with `uri`, or `data.url`: an uploaded / photo avatar.
/// - an emoji avatar: `AgentAvatarView`.
/// - anything else, or nothing saved: the Gizzi mascot (the web's fallback).
///
/// Pack art is often SVG (clawd-crew, duet-carry, gizzi-youtube), which
/// UIImage can't decode, so `AvatarArt` draws SVG with SwiftDraw. Every look
/// is trimmed to its drawn pixels and fitted at 88% of the box, the web's
/// avatar-framing, so no face is cropped and every bot reads the same size.
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
            PortraitAvatar(url: url, size: size)
                .accessibilityLabel("\(bot.botDisplayName) avatar")
        case .gizzi:
            SetupGizzi(size: size * 0.86, emotion: live ? .pleased : .focused)
                .frame(width: size, height: size)
                .accessibilityLabel("\(bot.botDisplayName) avatar")
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
    /// Kept for call sites; every look is now framed the same way.
    var tiled = false

    var body: some View {
        face
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

    @ViewBuilder
    private var face: some View {
        // Every look is trimmed to its drawn pixels and fitted the same way
        // (web avatar-framing), so no kind of art is drawn larger than its frame.
        BotAvatarView(bot: bot, size: size, live: live, working: working)
            .frame(width: size, height: size)
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
    /// The Gizzi mascot (web BotAvatar's GizziMascot), every new bot's default look.
    case gizzi
    /// A plain emoji / text avatar (AgentAvatarView draws it).
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
            // Mascot looks (web AgentMascotPreview): Gizzi, or the pack character
            // each other template stands for (mascot-characters.ts).
            if case .string(let type)? = object["type"], type == "mascot" {
                let template: String? = {
                    if case .string(let t)? = object["mascotTemplate"] { return t }
                    if case .object(let m)? = object["mascot"], case .string(let t)? = m["template"] { return t }
                    return nil
                }()
                if template == nil || template == "gizzi" { return .gizzi }
                if let art = mascotCharacters[template!] {
                    if let sheet = art.sheet, let url = webAssetURL(sheet) { return .sprite(url) }
                    if let portrait = art.portrait, let url = webAssetURL(portrait) { return .image(url) }
                }
            }
            // Any other look (colour/initials, vector pets): the web's LegacyBotAvatar
            // fallback is Gizzi, never a generated pattern.
            return .gizzi
        }
        if let avatar = bot.avatar?.trimmingCharacters(in: .whitespaces), !avatar.isEmpty {
            if avatar.hasPrefix("http") || avatar.hasPrefix("/avatar-packs/") || avatar.hasPrefix("data:image"),
               let url = webAssetURL(avatar) {
                return .image(url)
            }
            // An emoji or short glyph the person chose.
            if AgentAvatarView.gridSeed(avatar) == nil, !avatar.hasPrefix("{") { return .legacy }
        }
        // No look saved: every bot shares Gizzi (web BotAvatar fallback).
        return .gizzi
    }

    /// Pack sprites are stored as web paths ("/avatar-packs/…"), served by
    /// the web app; a bare path would never load on the phone.
    static func webAssetURL(_ raw: String) -> URL? {
        if raw.hasPrefix("/") { return URL(string: raw, relativeTo: webOrigin)?.absoluteURL }
        return URL(string: raw)
    }

    static let webOrigin = URL(string: "https://ai.allternit.com")!

    /// allternit-ai mascot-characters.ts MASCOT_CHARACTERS, resolved to each sprite's sheet and portrait.
    static let mascotCharacters: [String: (sheet: String?, portrait: String?)] = [
        "bot": ("/avatar-packs/alloy-classic/bolt-sheet.webp", "/avatar-packs/alloy-classic/bolt.webp"),
        "creature": ("/avatar-packs/creatures/fern-sheet.webp", "/avatar-packs/creatures/fern.webp"),
        "cyber": ("/avatar-packs/cyber/neon-district-sheet.webp", "/avatar-packs/cyber/neon-district.webp"),
        "data": ("/avatar-packs/space-corps/navigator-sheet.webp", "/avatar-packs/space-corps/navigator.webp"),
        "education": ("/avatar-packs/creatures/otis-sheet.webp", "/avatar-packs/creatures/otis.webp"),
        "fashion": ("/avatar-packs/creatures/milo-sheet.webp", "/avatar-packs/creatures/milo.webp"),
        "finance": (nil, "/avatar-packs/office-professionals/theo.png"),
        "food": (nil, "/avatar-packs/dot-friends/crumb.webp"),
        "gaming": ("/avatar-packs/retro-pixel/bitbuddy-sheet.webp", "/avatar-packs/retro-pixel/bitbuddy.webp"),
        "geometric": ("/avatar-packs/minimals/mark-sigma-sheet.webp", "/avatar-packs/minimals/mark-sigma.webp"),
        "healthcare": ("/avatar-packs/alloy-classic/fable-sheet.webp", "/avatar-packs/alloy-classic/fable.webp"),
        "legal": (nil, "/avatar-packs/office-professionals/kest.png"),
        "magic": ("/avatar-packs/magic/emberwhelp-sheet.webp", "/avatar-packs/magic/emberwhelp.webp"),
        "minimal": ("/avatar-packs/minimals/mark-omega-sheet.webp", "/avatar-packs/minimals/mark-omega.webp"),
        "music": ("/avatar-packs/dot-friends/pixel-sheet.webp", "/avatar-packs/dot-friends/pixel.webp"),
        "nature": ("/avatar-packs/nature/leafheart-sheet.webp", "/avatar-packs/nature/leafheart.webp"),
        "orb": ("/avatar-packs/orbs/plasma-sheet.webp", "/avatar-packs/orbs/plasma.webp"),
        "realEstate": ("/avatar-packs/alloy-classic/atlas-sheet.webp", "/avatar-packs/alloy-classic/atlas.webp"),
        "retail": ("/avatar-packs/dot-friends/servo-sheet.webp", "/avatar-packs/dot-friends/servo.webp"),
        "science": ("/avatar-packs/space-corps/science-officer-sheet.webp", "/avatar-packs/space-corps/science-officer.webp"),
        "security": ("/avatar-packs/mecha/riker-sheet.webp", "/avatar-packs/mecha/riker.webp"),
        "sports": ("/avatar-packs/dot-friends/hop-sheet.webp", "/avatar-packs/dot-friends/hop.webp"),
        "travel": ("/avatar-packs/space-corps/commander-sheet.webp", "/avatar-packs/space-corps/commander.webp")
    ]

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

/// Avatar art decoding and framing, shared by portraits, sprites and the
/// colour sampler (allternit-ai src/views/bots/avatar-framing.ts).
enum AvatarArt {
    /// Fetches a web asset, an https URL or a data: URL.
    nonisolated static func data(_ url: URL) async -> Data? {
        guard let (data, response) = try? await URLSession.shared.data(from: url) else { return nil }
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) { return nil }
        return data
    }

    /// Bitmaps through ImageIO; SVG drawn with SwiftDraw, its long side `svgPixels`.
    nonisolated static func decode(_ data: Data, url: URL, svgPixels: CGFloat = 1024) -> CGImage? {
        if isSVG(data, url: url) {
            guard let svg = SVG(data: data), svg.size.width > 0, svg.size.height > 0 else { return nil }
            let k = svgPixels / max(svg.size.width, svg.size.height)
            let size = CGSize(width: (svg.size.width * k).rounded(), height: (svg.size.height * k).rounded())
            return svg.rasterize(size: size, scale: 1).cgImage
        }
        return UIImage(data: data)?.cgImage
    }

    nonisolated static func isSVG(_ data: Data, url: URL) -> Bool {
        if url.pathExtension.lowercased() == "svg" { return true }
        if url.scheme == "data", url.absoluteString.hasPrefix("data:image/svg") { return true }
        let head = String(decoding: data.prefix(512), as: UTF8.self).lowercased()
        return head.contains("<svg")
    }

    /// RGBA pixels of a region, unpremultiplied enough for the content test.
    nonisolated private static func pixels(_ image: CGImage, _ rect: CGRect) -> [UInt8]? {
        let w = Int(rect.width), h = Int(rect.height)
        guard w > 0, h > 0, let region = image.cropping(to: rect) else { return nil }
        var buffer = [UInt8](repeating: 0, count: w * h * 4)
        let drawn = buffer.withUnsafeMutableBytes { raw -> Bool in
            guard let context = CGContext(data: raw.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                          space: CGColorSpaceCreateDeviceRGB(),
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            context.draw(region, in: CGRect(x: 0, y: 0, width: w, height: h))
            return true
        }
        return drawn ? buffer : nil
    }

    /// web avatarContentBounds: the box of the drawn pixels, ignoring transparent
    /// padding and light neutral paper, plus 1.5% padding. In image space
    /// (top-left origin), relative to `rect`.
    nonisolated static func contentBounds(_ image: CGImage, in rect: CGRect? = nil) -> CGRect {
        let area = rect ?? CGRect(x: 0, y: 0, width: image.width, height: image.height)
        let w = Int(area.width), h = Int(area.height)
        let whole = CGRect(x: 0, y: 0, width: area.width, height: area.height)
        guard let px = pixels(image, area) else { return whole }
        var left = w, top = h, right = -1, bottom = -1
        for y in 0..<h {
            // A bitmap context's memory starts at the image's top row.
            let row = y * w * 4
            for x in 0..<w {
                let i = row + x * 4
                let a = Int(px[i + 3])
                if a < 40 { continue }
                let r = Int(px[i]) * 255 / a, g = Int(px[i + 1]) * 255 / a, b = Int(px[i + 2]) * 255 / a
                let lo = min(r, g, b), hi = max(r, g, b)
                if lo > 205 && hi - lo < 28 { continue }
                left = min(left, x); right = max(right, x)
                top = min(top, y); bottom = max(bottom, y)
            }
        }
        guard right >= left, bottom >= top else { return whole }
        let pad = Int((Double(max(w, h)) * 0.015).rounded(.up))
        left = max(0, left - pad); top = max(0, top - pad)
        right = min(w - 1, right + pad); bottom = min(h - 1, bottom + pad)
        return CGRect(x: left, y: top, width: right - left + 1, height: bottom - top + 1)
    }

    /// A crop with its own pixels, so the full sheet or raster can be freed.
    nonisolated static func copy(_ image: CGImage, _ rect: CGRect) -> CGImage? {
        guard let region = image.cropping(to: rect.integral) else { return nil }
        let w = region.width, h = region.height
        guard let context = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                      space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        context.draw(region, in: CGRect(x: 0, y: 0, width: w, height: h))
        return context.makeImage()
    }
}

/// CGImages cross the detached decode task; they are immutable.
private struct SpriteFrames: @unchecked Sendable {
    let images: [CGImage]
    /// The resting frame's drawn box inside the frames (web spriteDestination
    /// `neutral`), which sets the character's size and position.
    let neutral: CGRect
}

/// Frames of a sprite sheet, cached per URL.
@MainActor
private final class SpriteFrameCache {
    static let shared = SpriteFrameCache()
    private var frames: [URL: SpriteFrames] = [:]
    private var loading: [URL: Task<SpriteFrames, Never>] = [:]

    /// The idle loop of a pet sheet, cut like the web's AnimatedBotSprite:
    /// cells are 192×208 (×k on a 2× sheet; sheets have 9 or more rows, so
    /// the height is never divided by a row count), row 0 is idle, blank
    /// cells at the row's end are skipped (they flashed as a blink). Every
    /// frame is cut to the union of the row's drawn boxes, so a raised arm
    /// is drawn instead of cropped, and the resting frame's box is kept to
    /// size and place the character (web spriteDestination).
    nonisolated static func idleFrames(of image: CGImage) -> SpriteFrames {
        let empty = SpriteFrames(images: [], neutral: .zero)
        let columns = 8
        let k = max(1, image.width / (columns * 192))
        let cellW = 192 * k, cellH = 208 * k
        guard image.width >= cellW * columns, image.height >= cellH else { return empty }
        let cell = CGRect(x: 0, y: 0, width: cellW, height: cellH)
        var boxes: [(column: Int, box: CGRect)] = []
        for column in 0..<columns {
            let rect = cell.offsetBy(dx: CGFloat(column * cellW), dy: 0)
            let box = AvatarArt.contentBounds(image, in: rect)
            // A blank cell measures as the whole cell.
            if box.size != cell.size { boxes.append((column, box)) }
        }
        guard let first = boxes.first else { return empty }
        let union = boxes.dropFirst().reduce(first.box) { $0.union($1.box) }
        let images = boxes.compactMap { AvatarArt.copy(image, union.offsetBy(dx: CGFloat($0.column * cellW), dy: 0)) }
        let neutral = first.box.offsetBy(dx: -union.minX, dy: -union.minY)
        return SpriteFrames(images: images, neutral: neutral)
    }

    /// Frames already loaded, so a redrawn avatar starts on its face
    /// instead of blank (it blinked on every parent refresh).
    fileprivate func cached(_ url: URL) -> SpriteFrames? { frames[url] }

    fileprivate func frames(for url: URL) async -> SpriteFrames? {
        if let cached = frames[url] { return cached }
        if let running = loading[url] { let result = await running.value; return result.images.isEmpty ? nil : result }
        let task = Task.detached(priority: .utility) { () -> SpriteFrames in
            guard let data = await AvatarArt.data(url),
                  let image = AvatarArt.decode(data, url: url) else { return SpriteFrames(images: [], neutral: .zero) }
            return SpriteFrameCache.idleFrames(of: image)
        }
        loading[url] = task
        let result = await task.value
        loading[url] = nil
        // A failed load isn't kept, so the next appearance tries again.
        guard !result.images.isEmpty else { return nil }
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
    @State private var sprite: SpriteFrames?
    @ReducedMotion private var reduceMotion

    init(url: URL, size: CGFloat, live: Bool) {
        self.url = url
        self.size = size
        self.live = live
        _sprite = State(initialValue: SpriteFrameCache.shared.cached(url))
    }

    var body: some View {
        Group {
            if let sprite, !sprite.images.isEmpty {
                let frames = sprite.images
                if live && !reduceMotion && frames.count > 1 {
                    TimelineView(.periodic(from: .now, by: 0.12)) { context in
                        let index = Int(context.date.timeIntervalSinceReferenceDate / 0.12) % frames.count
                        placed(frames[index], neutral: sprite.neutral)
                    }
                } else {
                    placed(frames[0], neutral: sprite.neutral)
                }
            } else {
                Color.clear
            }
        }
        .frame(width: size, height: size)
        .task(id: url) {
            if let loaded = await SpriteFrameCache.shared.frames(for: url) { sprite = loaded }
        }
    }

    /// web spriteDestination: the resting frame fills 88% of the box, centred;
    /// the row is drawn around it, shrunk only if it would leave the box.
    private func placed(_ image: CGImage, neutral: CGRect) -> some View {
        let rowW = CGFloat(image.width), rowH = CGFloat(image.height)
        let base = size * 0.88 / max(neutral.width, neutral.height, 1)
        let scale = min(base, size / max(rowW, 1), size / max(rowH, 1))
        let width = rowW * scale, height = rowH * scale
        let nx = (size - neutral.width * scale) / 2, ny = (size - neutral.height * scale) / 2
        let x = min(max(nx - neutral.minX * scale, 0), size - width)
        let y = min(max(ny - neutral.minY * scale, 0), size - height)
        return Image(decorative: image, scale: 1)
            .resizable()
            .interpolation(.high)
            .frame(width: width, height: height)
            .offset(x: x, y: y)
            .frame(width: size, height: size, alignment: .topLeading)
    }
}

/// Trimmed portrait art, cached per URL.
@MainActor
private final class PortraitCache {
    static let shared = PortraitCache()
    /// nil: the art failed to load or decode.
    private var images: [URL: CGImage?] = [:]
    private var loading: [URL: Task<PortraitImage, Never>] = [:]

    fileprivate func cached(_ url: URL) -> CGImage?? { images[url] }

    fileprivate func image(for url: URL) async -> CGImage? {
        if let cached = images[url] { return cached }
        if let running = loading[url] { return await running.value.image }
        let task = Task.detached(priority: .utility) { () -> PortraitImage in
            guard let data = await AvatarArt.data(url), let image = AvatarArt.decode(data, url: url) else {
                return PortraitImage(image: nil, failed: true)
            }
            let box = AvatarArt.contentBounds(image)
            return PortraitImage(image: AvatarArt.copy(image, box) ?? image, failed: false)
        }
        loading[url] = task
        let result = await task.value
        loading[url] = nil
        // Network failures retry on the next appearance; undecodable art stays failed.
        if result.image != nil { images[url] = .some(result.image) }
        return result.image
    }
}

private struct PortraitImage: @unchecked Sendable {
    let image: CGImage?
    let failed: Bool
}

/// A pack portrait, upload or photo sprite (web AvatarPortraitCanvas): SVG or
/// bitmap, trimmed to its drawn pixels and fitted at 88% of the box, never
/// cropped. Gizzi stands in only when the art can't be loaded.
struct PortraitAvatar: View {
    let url: URL
    let size: CGFloat
    @State private var image: CGImage?
    @State private var failed = false

    init(url: URL, size: CGFloat) {
        self.url = url
        self.size = size
        _image = State(initialValue: PortraitCache.shared.cached(url) ?? nil)
    }

    var body: some View {
        Group {
            if let image {
                Image(decorative: image, scale: 1)
                    .resizable()
                    .interpolation(.high)
                    .scaledToFit()
                    .frame(width: size * 0.88, height: size * 0.88)
            } else if failed {
                SetupGizzi(size: size * 0.86, emotion: .focused)
            } else {
                Color.clear
            }
        }
        .frame(width: size, height: size)
        .task(id: url) {
            failed = false
            if let loaded = await PortraitCache.shared.image(for: url) { image = loaded } else { failed = true }
        }
    }
}

// MARK: - Bot presence (Allo motion kit)

/// What the bot is doing on the phone. Wave 2 only passes this.
enum BotPresence: Equatable {
    case idle        // bob
    case suitUp      // call went live: the bot puts the headset on
    case talking     // bot speaking: head tilt + mic pulse
    case listening   // on a call, other side speaking: bob, headset on, mic still
    case suitDown    // call ended: the bot takes the headset off

    var headsetOn: Bool { self == .suitUp || self == .talking || self == .listening }
}

/// Headset v2 colors (HEADSET-V2.md): graphite with a top highlight, a lighter
/// graphite in dark mode, and a darker-tan cushion where the cup sits on the head.
private enum HeadsetInk {
    static func body(_ dark: Bool) -> Color { dark ? Color(hex: "#5C626D") : Color(hex: "#2B2E35") }
    static func highlight(_ dark: Bool) -> Color { dark ? Color(hex: "#9AA1AD") : Color(hex: "#4A4F59") }
    static let cushion = Color(hex: "#B8946E")
    static let beacon = Color(hex: "#D97757")
}

/// Gizzi wearing the headset. GizziMascot.png is 264 px for the web's 96-unit
/// art box (2.75 px per unit, zero offset: beacon at px 121-142 x 22-35 = units
/// 44-52 x 8-13), so the web coordinates apply 1:1 over the image once it is
/// drawn `art` pt wide. Cups sit under the image (they show at the head's
/// sides), the band and cushions over it, the mic boom last. All of it lives in
/// the one container that bobs, tilts and squashes, so nothing floats on top.
private struct GizziHeadsetArt: View {
    var art: CGFloat
    var wear: Bool
    var boomDown: Bool
    var landing: Bool
    var talking: Bool
    var micScale: CGFloat
    var dark: Bool
    var bodyY: CGFloat
    var tilt: Double

    private func rect(_ k: CGFloat, _ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ r: CGFloat, _ c: Color) -> some View {
        RoundedRectangle(cornerRadius: r * k, style: .continuous).fill(c)
            .frame(width: w * k, height: h * k)
            .position(x: (x + w / 2) * k, y: (y + h / 2) * k)
    }

    var body: some View {
        let k = art / 96
        let hs = HeadsetInk.body(dark), hi = HeadsetInk.highlight(dark)
        let drop: CGFloat = wear ? 0 : -20 * k
        ZStack {
            // Cups (under the head, so only the sides show).
            ZStack {
                rect(k, 11, 26, 10, 16, 3, hs)
                rect(k, 75, 26, 10, 16, 3, hs)
            }
            .scaleEffect(x: wear ? 1 : 1.22, y: 1)
            .offset(y: drop).opacity(wear ? 1 : 0)

            Image("GizziMascot").resizable().interpolation(.high)

            // Cushions: the cup edge touching the head.
            ZStack {
                rect(k, 19, 27, 3, 14, 1, HeadsetInk.cushion)
                rect(k, 74, 27, 3, 14, 1, HeadsetInk.cushion)
            }
            .scaleEffect(x: wear ? 1 : 1.22, y: 1)
            .offset(y: drop).opacity(wear ? 1 : 0)

            // Band over the antennae, with the beacon redrawn on top.
            ZStack {
                rect(k, 28, 9, 40, 5, 2, hs)
                rect(k, 30, 9, 36, 1.4, 0.7, hi)
                rect(k, 20, 12, 10, 5, 2, hs)
                rect(k, 66, 12, 10, 5, 2, hs)
                rect(k, 14, 16, 7, 11, 2, hs)
                rect(k, 75, 16, 7, 11, 2, hs)
                rect(k, 44, 8, 8, 5, 2, HeadsetInk.beacon)
            }
            .offset(y: drop).opacity(wear ? 1 : 0)

            // Mic boom, hinged just under the left cup's top; swings down to the mouth.
            ZStack {
                rect(k, 17, 40, 4, 14, 1.5, hs)
                rect(k, 18, 52, 17, 3.5, 1.5, hs)
                rect(k, 33, 50.5, 7, 6.5, 2.5, talking ? Theme.statusSuccess : hs)
                    .scaleEffect(micScale)
            }
            .rotationEffect(.degrees(boomDown ? 0 : -95), anchor: UnitPoint(x: 19.0 / 96, y: 41.0 / 96))
            .offset(y: drop).opacity(wear ? 1 : 0)
        }
        .frame(width: art, height: art)
        .scaleEffect(x: landing ? 1.03 : 1, y: landing ? 0.95 : 1, anchor: .bottom)
        .offset(y: bodyY * k)
        .rotationEffect(.degrees(tilt), anchor: .bottom)
    }
}

/// The corner badge other bots get instead of art over their face (38% of the
/// avatar, bottom-right, ring in the surface color). Its dot pulses while talking.
private struct HeadsetBadge: View {
    var box: CGFloat
    var talking: Bool
    var pulse: CGFloat
    var dark: Bool

    var body: some View {
        let d = box * 0.38
        ZStack {
            Circle().fill(HeadsetInk.body(dark))
            Canvas { ctx, sz in
                let s = sz.width / 24
                var arc = Path()
                arc.move(to: CGPoint(x: 4 * s, y: 14 * s))
                arc.addLine(to: CGPoint(x: 4 * s, y: 12 * s))
                arc.addArc(center: CGPoint(x: 12 * s, y: 12 * s), radius: 8 * s,
                           startAngle: .degrees(180), endAngle: .degrees(0), clockwise: false)
                arc.addLine(to: CGPoint(x: 20 * s, y: 14 * s))
                ctx.stroke(arc, with: .color(.white), style: StrokeStyle(lineWidth: 2.4 * s, lineCap: .round))
                for x in [3.0, 17.0] {
                    ctx.fill(Path(roundedRect: CGRect(x: x * s, y: 13 * s, width: 4 * s, height: 7 * s), cornerRadius: 1.5 * s), with: .color(.white))
                }
                let dotR = 2 * s * pulse
                ctx.fill(Path(ellipseIn: CGRect(x: 12 * s - dotR, y: 21 * s - dotR, width: dotR * 2, height: dotR * 2)),
                         with: .color(talking ? Theme.statusSuccess : Color(hex: "#9AA0AA")))
            }
            .frame(width: d * 0.62, height: d * 0.62)
        }
        .frame(width: d, height: d)
        .overlay(Circle().stroke(Color("BgPanel"), lineWidth: 2))
        .offset(x: 2, y: 2)
        .accessibilityHidden(true)
    }
}

private extension View {
    @ViewBuilder func presenceMatched(_ ns: Namespace.ID?, id: String) -> some View {
        if let ns { matchedGeometryEffect(id: id, in: ns) } else { self }
    }
}

/// One wrapper around a bot's avatar for the bot phone: bob and talk tilt on the
/// container, a glance toward a new message (Gizzi only), and shrink-to-chip
/// when a caller passes a namespace. Gizzi WEARS the headset (drawn in the same
/// transformed container as the art); every other look gets a corner badge.
/// Gizzi is a raster asset on iOS, so its eyes and mouth can't move: the
/// glance-up and `://` squash from the web are left out. Static under Reduce Motion.
struct BotPresenceView: View {
    let bot: AgentRecord
    var size: CGFloat = 40
    var presence: BotPresence = .idle
    /// A new message arrived on this side (-1 left ... 1 right). Gizzi turns toward it for 1.2 s.
    var lookAt: CGFloat? = nil
    /// Pass a namespace to make the avatar a shared element (big face to chip).
    var namespace: Namespace.ID? = nil
    var matchedId: String = "botPresenceFace"

    @ReducedMotion private var reduceMotion
    @Environment(\.colorScheme) private var scheme
    @State private var glance: CGFloat = 0
    @State private var wear: Bool
    @State private var boomDown: Bool
    @State private var landing = false
    @State private var sequence = 0

    init(bot: AgentRecord, size: CGFloat = 40, presence: BotPresence = .idle, lookAt: CGFloat? = nil,
         namespace: Namespace.ID? = nil, matchedId: String = "botPresenceFace") {
        self.bot = bot; self.size = size; self.presence = presence; self.lookAt = lookAt
        self.namespace = namespace; self.matchedId = matchedId
        // A bot already mid-call has the headset on; only .suitUp plays the put-on.
        let on = presence.headsetOn && presence != .suitUp
        _wear = State(initialValue: on)
        _boomDown = State(initialValue: on)
    }

    // Springs from the web (stiffness/damping, mass 1): 520/30, 300/16, 520/26.
    private static let drop = Animation.spring(response: 0.275, dampingFraction: 0.66)
    private static let swing = Animation.spring(response: 0.363, dampingFraction: 0.46)
    private static let badgeIn = Animation.spring(response: 0.275, dampingFraction: 0.57)

    var body: some View {
        let box = size < 36 ? max(size, 34) : size
        Group {
            if reduceMotion {
                content(box: box, t: 0)
            } else {
                TimelineView(.animation(minimumInterval: 1.0 / 30)) { tl in
                    content(box: box, t: tl.date.timeIntervalSinceReferenceDate)
                }
            }
        }
        .frame(width: box, height: box)
        .presenceMatched(namespace, id: matchedId)
        .onAppear { if presence == .suitUp { putOn() } }
        .onChange(of: presence.headsetOn) { _, on in on ? putOn() : takeOff() }
        .onChange(of: lookAt) { _, new in
            guard let new, !reduceMotion, BotAvatarSource.resolve(bot) == .gizzi else { return }
            withAnimation(.easeOut(duration: 0.25)) { glance = new }
            Task {
                try? await Task.sleep(nanoseconds: 1_200_000_000)
                withAnimation(.easeOut(duration: 0.25)) { glance = 0 }
            }
        }
    }

    private func apply(_ animation: Animation?, _ change: @escaping () -> Void) {
        if reduceMotion {
            var tx = Transaction(); tx.disablesAnimations = true
            withTransaction(tx, change)
        } else {
            withAnimation(animation, change)
        }
    }

    /// Put on (no spin): band and cups drop with a spring, a small squash on landing,
    /// then the mic boom swings down.
    private func putOn() {
        sequence += 1
        let mine = sequence
        apply(Self.drop) { wear = true }
        apply(Self.swing.delay(0.3)) { boomDown = true }
        guard !reduceMotion else { return }
        Task {
            try? await Task.sleep(nanoseconds: 260_000_000)
            guard mine == sequence else { return }
            withAnimation(.linear(duration: 0.09)) { landing = true }
            try? await Task.sleep(nanoseconds: 100_000_000)
            guard mine == sequence else { return }
            withAnimation(.easeOut(duration: 0.12)) { landing = false }
        }
    }

    /// Take off (no spin): the mic flips up first, then band and cups rise and fade.
    private func takeOff() {
        sequence += 1
        landing = false
        apply(.easeIn(duration: 0.16)) { boomDown = false }
        apply(Animation.easeIn(duration: 0.28).delay(0.12)) { wear = false }
    }

    private func content(box: CGFloat, t: Double) -> some View {
        let talking = presence == .talking
        let isGizzi = BotAvatarSource.resolve(bot) == .gizzi
        let dark = scheme == .dark
        var y = 0.0, tilt = 0.0, micScale = 1.0, pulse = 1.0
        if !reduceMotion {
            if talking {
                // Gizzi: y 0, -1.2, 0 and rotate 0, -1.5, 1, 0 over 0.8 s. Others keep the 0.9 s tilt.
                let period = isGizzi ? 0.8 : 0.9
                let p = (t / period).truncatingRemainder(dividingBy: 1)
                if isGizzi {
                    tilt = p < 1.0 / 3 ? -1.5 * (p * 3) : p < 2.0 / 3 ? -1.5 + 2.5 * (p * 3 - 1) : 1 - (p * 3 - 2)
                    y = -1.2 * sin(.pi * p)
                } else {
                    tilt = p < 1.0 / 3 ? -9 * p : p < 2.0 / 3 ? -3 + 15 * (p - 1.0 / 3) : 2 - 6 * (p - 2.0 / 3)
                    y = -1.5 * sin(.pi * p)
                }
                let m = sin(.pi * (t / 0.6).truncatingRemainder(dividingBy: 1))   // 0.6 s mic / dot loop
                micScale = 1 + 0.25 * m
                pulse = 1 + 0.4 * m
            } else {
                y = (isGizzi ? -1.6 : -2) * (1 - cos(2 * .pi * t / 2.4)) / 2     // 2.4 s bob
            }
        }
        let k = box / 96
        return Group {
            if isGizzi {
                // The raster art is drawn at 86% of the box, centred (SetupGizzi's fit).
                GizziHeadsetArt(art: box * 0.86, wear: wear, boomDown: boomDown, landing: landing,
                                talking: talking, micScale: micScale, dark: dark,
                                bodyY: y * 0.86, tilt: tilt)
                    .frame(width: box, height: box)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(wear ? "\(bot.botDisplayName) avatar, wearing a headset" : "\(bot.botDisplayName) avatar")
            } else {
                ZStack(alignment: .bottomTrailing) {
                    BotAvatarView(bot: bot, size: box, live: presence != .idle)
                        .offset(y: y * k)
                        .rotationEffect(.degrees(tilt))
                    if presence.headsetOn {
                        HeadsetBadge(box: box, talking: talking, pulse: pulse, dark: dark)
                            .transition(reduceMotion ? .identity : .scale(scale: 0.4).combined(with: .opacity))
                    }
                }
                .frame(width: box, height: box)
                .animation(reduceMotion ? nil : Self.badgeIn, value: presence.headsetOn)
            }
        }
        .offset(x: glance * 3 * k)
    }
}
