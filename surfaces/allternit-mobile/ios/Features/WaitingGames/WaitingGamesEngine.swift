import SwiftUI
import UIKit

/// The waiting games (web waiting-games-engine.ts): eight small games drawn
/// in ASCII on one canvas, played while a bot works (WaitingGamesLayer).
/// Each game keeps its own state and has init/step/draw plus input
/// handlers; the engine owns the character grid and the clock, and reports
/// the score line and the banner back to the window. The game bodies follow
/// the web port of the prototype (artifacts.allternit.com/Waiting-games.html)
/// line for line, so rules, speeds and scoring match.
enum WaitingGameID: String, CaseIterable, Identifiable {
    case fish, race, `catch`, dodge, bash, stack, snake, memory
    var id: String { rawValue }

    var name: String {
        switch self {
        case .fish: return "Big Fish"
        case .race: return "Bot Race"
        case .catch: return "Token Catch"
        case .dodge: return "Meteor Dodge"
        case .bash: return "Bug Bash"
        case .stack: return "Stack Deploy"
        case .snake: return "Context Snake"
        case .memory: return "Bot Memory"
        }
    }
}

struct WaitingGameRacer {
    let name: String
    var color: String?
}

struct WaitingGameBanner: Equatable {
    let title: String
    let sub: String
}

// MARK: - Drawing

private let TAU = Double.pi * 2
private func rnd(_ a: Double, _ b: Double) -> Double { a + Double.random(in: 0..<1) * (b - a) }
/// JS `%` on numbers (keeps the sign of the dividend).
private func jmod(_ a: Double, _ b: Double) -> Double { fmod(a, b) }

/// Colours by hex, cached: the games draw hundreds of glyphs a frame.
private enum Ink {
    static var cache: [String: Color] = [:]
    static func color(_ hex: String) -> Color {
        if let c = cache[hex] { return c }
        let c = Color(hex: hex)
        cache[hex] = c
        return c
    }
}

/// The canvas calls the games make (txt, ptxt, bg, rects), on a SwiftUI
/// GraphicsContext with the engine's grid metrics.
struct WaitingGamesPainter {
    var ctx: GraphicsContext
    let W: Double, H: Double, cw: Double, chh: Double, FS: Double

    private func mono(_ size: Double, bold: Bool = false) -> Font {
        .system(size: size, weight: bold ? .semibold : .regular, design: .monospaced)
    }

    func bg(_ stops: [String]) {
        let g = Gradient(colors: stops.map(Ink.color))
        ctx.fill(Path(CGRect(x: 0, y: 0, width: W, height: H)),
                 with: .linearGradient(g, startPoint: .zero, endPoint: CGPoint(x: 0, y: H)))
    }

    /// One string on the grid, a glyph per cell (spaces skipped), top-left
    /// at column x, row y.
    func txt(_ x: Double, _ y: Double, _ s: String, _ col: String, _ a: Double = 1) {
        guard a > 0.004 else { return }
        let color = Ink.color(col).opacity(min(1, a))
        var i = 0.0
        for ch in s {
            if ch != " " {
                ctx.draw(Text(String(ch)).font(mono(FS)).foregroundColor(color),
                         at: CGPoint(x: (x + i) * cw, y: y * chh + FS * 0.1), anchor: .topLeading)
            }
            i += 1
        }
    }

    /// Centred text at a pixel point (the newer games' ptxt).
    func ptxt(_ px: Double, _ py: Double, _ s: String, _ col: String, _ a: Double = 1, _ size: Double? = nil) {
        guard a > 0.004 else { return }
        ctx.draw(Text(s).font(mono(size ?? FS, bold: size != nil)).foregroundColor(Ink.color(col).opacity(min(1, a))),
                 at: CGPoint(x: px, y: py), anchor: .center)
    }

    /// A centred line in the system face (Big Fish's flash).
    func sans(_ px: Double, _ py: Double, _ s: String, _ col: String, _ a: Double, _ size: Double) {
        guard a > 0.004 else { return }
        ctx.draw(Text(s).font(.system(size: size, weight: .semibold)).foregroundColor(Ink.color(col).opacity(min(1, a))),
                 at: CGPoint(x: px, y: py), anchor: .top)
    }

    /// A glyph at a pixel point, top-leading (Bot Race's big word).
    func glyph(_ px: Double, _ py: Double, _ s: String, _ col: String, _ size: Double, bold: Bool, _ a: Double = 1) {
        ctx.draw(Text(s).font(mono(size, bold: bold)).foregroundColor(Ink.color(col).opacity(a)),
                 at: CGPoint(x: px, y: py), anchor: .topLeading)
    }

    func rect(_ x: Double, _ y: Double, _ w: Double, _ h: Double, _ color: Color) {
        ctx.fill(Path(CGRect(x: x, y: y, width: w, height: h)), with: .color(color))
    }

    func roundRect(_ x: Double, _ y: Double, _ w: Double, _ h: Double, _ r: Double, _ color: Color) {
        ctx.fill(Path(roundedRect: CGRect(x: x, y: y, width: w, height: h), cornerRadius: r), with: .color(color))
    }

    static func monoWidth(_ size: Double, bold: Bool = false) -> Double {
        let font = UIFont.monospacedSystemFont(ofSize: size, weight: bold ? .semibold : .regular)
        return Double(("M" as NSString).size(withAttributes: [.font: font]).width)
    }
}

// MARK: - Engine

/// Owns the grid, the clock and the current game; the window draws it in a
/// TimelineView and forwards touches and keys.
@MainActor
final class WaitingGamesEngine: ObservableObject {
    @Published private(set) var hud: [String] = []
    @Published private(set) var banner: WaitingGameBanner? = nil

    fileprivate(set) var W = 0.0, H = 0.0, cw = 8.0, chh = 14.0, C = 0.0, R = 0.0, FS = 13.0
    @Published private(set) var playing = true
    private var clock = 0.0
    private var last: Date? = nil
    private var game: WaitingGame? = nil
    private var needsInit = false
    private var games: [WaitingGameID: WaitingGame] = [:]

    static let bestKey = "allternit.waitingGames.best"

    init(racers: [WaitingGameRacer]) {
        games = [
            .fish: FishGame(self), .race: RaceGame(self, racers: racers), .catch: CatchGame(self),
            .dodge: DodgeGame(self), .bash: BugBashGame(self), .stack: StackGame(self),
            .snake: SnakeGame(self), .memory: MemoryGame(self),
        ]
        // Best scores are a nicety: a JSON object of game id → best.
        if let raw = UserDefaults.standard.string(forKey: Self.bestKey),
           let data = raw.data(using: .utf8),
           let saved = try? JSONSerialization.jsonObject(with: data) as? [String: Double] {
            for (id, g) in games { if let b = saved[id.rawValue] { g.best = b } }
        }
    }

    func saveBest() {
        var out: [String: Double] = [:]
        for (id, g) in games { if let b = g.best { out[id.rawValue] = b } }
        if let data = try? JSONSerialization.data(withJSONObject: out), let s = String(data: data, encoding: .utf8) {
            UserDefaults.standard.set(s, forKey: Self.bestKey)
        }
    }

    // UI reports, posted after the frame so SwiftUI isn't updated mid-render.
    fileprivate func setHud(_ items: [String]) {
        guard items != hud else { return }
        DispatchQueue.main.async { [weak self] in self?.hud = items }
    }

    fileprivate func showBanner(_ title: String, _ sub: String = "") {
        let next = WaitingGameBanner(title: title, sub: sub)
        DispatchQueue.main.async { [weak self] in self?.banner = next }
    }

    fileprivate func hideBanner() {
        DispatchQueue.main.async { [weak self] in self?.banner = nil }
    }

    func pick(_ id: WaitingGameID) {
        objectWillChange.send()
        game = games[id]
        hideBanner()
        setHud([])
        if W > 0 { game?.initGame() } else { needsInit = true }
    }

    var hint: String { game?.hint ?? "" }
    var wantsKeyboard: Bool { game?.keyboard ?? false }

    func pointer(_ x: Double, _ y: Double) { if playing { game?.pointer(x, y) } }
    func click(_ x: Double, _ y: Double) { if playing { game?.click(x, y) } }

    /// A key the game took ("ArrowUp", "Enter", "Backspace", " " or a letter).
    @discardableResult
    func key(_ key: String) -> Bool { playing ? (game?.key(key) ?? false) : false }

    func setPlaying(_ on: Bool) {
        guard on != playing else { return }
        playing = on
        last = nil
        if !on { saveBest() }
    }

    /// Fits the grid to the canvas (web `fit`).
    private func fit(_ size: CGSize) {
        W = Double(size.width); H = Double(size.height)
        FS = W < 520 ? 11 : 13
        cw = WaitingGamesPainter.monoWidth(FS)
        chh = FS * 1.2
        C = (W / cw).rounded(.up); R = (H / chh).rounded(.up)
        game?.resize()
    }

    /// One frame: step the game by the time since the last frame, then draw.
    func render(_ ctx: inout GraphicsContext, size: CGSize, date: Date) {
        if size.width < 1 || size.height < 1 { return }
        if Double(size.width) != W || Double(size.height) != H { fit(size) }
        if needsInit, let game { needsInit = false; game.initGame() }
        var dt = 0.0
        if playing {
            if let last { dt = min(0.05, date.timeIntervalSince(last)) }
            last = date
            clock += dt
        }
        guard let game else { return }
        if playing { game.step(dt, clock) }
        var painter = WaitingGamesPainter(ctx: ctx, W: W, H: H, cw: cw, chh: chh, FS: FS)
        game.draw(&painter, clock)
    }

    /// Seconds on a monotonic clock (performance.now() / 1000).
    fileprivate var now: Double { CACurrentMediaTime() }
}

// MARK: - Games

@MainActor
private class WaitingGame {
    unowned let e: WaitingGamesEngine
    var best: Double? = nil
    var hint: String { "" }
    var keyboard: Bool { false }
    var flash: (t: Double, txt: String, c: String)? = nil

    init(_ e: WaitingGamesEngine) { self.e = e }

    var W: Double { e.W }
    var H: Double { e.H }
    var C: Double { e.C }
    var R: Double { e.R }
    var cw: Double { e.cw }
    var chh: Double { e.chh }
    var playRows: Double { e.R - 2 }

    func initGame() {}
    func resize() {}
    func pointer(_ px: Double, _ py: Double) {}
    func click(_ px: Double, _ py: Double) {}
    func key(_ k: String) -> Bool { false }
    func step(_ dt: Double, _ t: Double) {}
    func draw(_ p: inout WaitingGamesPainter, _ t: Double) {}

    func flashDraw(_ p: WaitingGamesPainter, _ t: Double) {
        if let f = flash, t - f.t < 1.4 { p.ptxt(W / 2, H * 0.28, f.txt, f.c, 1 - (t - f.t) / 1.4, 20) }
    }
}

// ---------- Big Fish: your finger is a fish. Eat smaller fish, avoid bigger ones; eaten = respawn small.
private let MIRROR: [Character: Character] = [">": "<", "<": ">", "(": ")", ")": "(", "/": "\\", "\\": "/"]
private func flip(_ s: String) -> String { String(s.reversed().map { MIRROR[$0] ?? $0 }) }
private func fishShape(_ n: Int) -> [String] {
    let mid = "><" + String(repeating: "(", count: n) + "°>"
    if n < 4 { return [mid] }
    return ["  " + String(repeating: "_", count: n) + "  ", mid, "  " + String(repeating: "‾", count: n) + "  "]
}
private let FISH_COL = ["#fde68a", "#fbbf24", "#fb923c", "#f87171", "#e879f9", "#94a3b8", "#cbd5e1"]

private final class FishGame: WaitingGame {
    struct Me { var x, y, tx, ty: Double; var size: Int; var xp: Int; var dir: Double; var inv: Double }
    struct Npc { var x, y, v, dir: Double; var s: Int; var p: Double; var dead = false }
    struct Kelp { let x, h, p: Double }
    struct Bub { let x, y, v: Double }
    var me = Me(x: 0, y: 0, tx: 0, ty: 0, size: 1, xp: 0, dir: 1, inv: 0)
    var npc: [Npc] = []
    var kelp: [Kelp] = []
    var bub: [Bub] = []
    var score = 0

    override var hint: String { "Move your finger to swim. Eat smaller fish to grow; stay away from bigger ones. Get eaten and you start small again." }

    override func initGame() {
        reset(true)
        best = best ?? 0
        kelp = (0..<14).map { _ in Kelp(x: Double.random(in: 0..<1), h: rnd(0.3, 0.75), p: rnd(0, TAU)) }
        bub = (0..<40).map { _ in Bub(x: Double.random(in: 0..<1), y: Double.random(in: 0..<1), v: rnd(1.5, 3.5)) }
    }

    func reset(_ first: Bool) {
        me = Me(x: C / 2, y: R / 2, tx: C / 2, ty: R / 2, size: 1, xp: 0, dir: 1, inv: first ? 0 : 2)
        if first { score = 0 }
        npc = []
        for _ in 0..<14 { spawn(true) }
    }

    func spawn(_ any: Bool) {
        let big = Double.random(in: 0..<1) < 0.22
        let size = Double(me.size)
        let s = max(1, min(7, Int((big ? rnd(size + 1, size + 4) : rnd(1, size + 0.6)).rounded())))
        let dir: Double = Double.random(in: 0..<1) < 0.5 ? 1 : -1
        npc.append(Npc(x: any ? rnd(0, C) : (dir > 0 ? -12 : C + 4), y: rnd(3, R - 4),
                       v: rnd(3, 8) * (s > 5 ? 0.7 : 1) / Double(s).squareRoot(), dir: dir, s: s, p: rnd(0, TAU)))
    }

    override func pointer(_ px: Double, _ py: Double) { me.tx = px / cw; me.ty = py / chh }

    override func step(_ dt: Double, _ t: Double) {
        let dx = me.tx - me.x, dy = me.ty - me.y
        me.x += dx * min(1, dt * 6); me.y += dy * min(1, dt * 6)
        if abs(dx) > 0.3 { me.dir = dx > 0 ? 1 : -1 }
        me.inv = max(0, me.inv - dt)
        let ms = fishShape(me.size)
        let mw = Double(ms[min(1, ms.count - 1)].count), mh = Double(ms.count)
        for i in npc.indices {
            npc[i].x += npc[i].v * npc[i].dir * dt
            npc[i].y += sin(t * 1.3 + npc[i].p) * dt * 0.6
            let f = npc[i]
            let sh = fishShape(f.s)
            let fw = Double(sh[min(1, sh.count - 1)].count), fh = Double(sh.count)
            // flee or chase a little
            let ddx = me.x - f.x, ddy = me.y - f.y, dist = hypot(ddx / 2, ddy)
            if dist < 10 {
                if f.s < me.size { npc[i].y -= sign(ddy) * dt * 3 }
                else if f.s > me.size { npc[i].y += sign(ddy) * dt * 1.5 }
            }
            let hit = abs(me.x - (f.x + fw / 2)) < (mw + fw) / 2 - 1 && abs(me.y - (f.y + fh / 2)) < (mh + fh) / 2 - 0.3
            if hit && !f.dead {
                if f.s < me.size {
                    npc[i].dead = true
                    score += f.s * 10
                    me.xp += f.s
                    if me.xp >= me.size * 4 && me.size < 7 { me.size += 1; me.xp = 0; flash = (t, "Bigger!", "#ffffff") }
                } else if f.s > me.size && me.inv <= 0 {
                    best = max(best ?? 0, Double(score))
                    flash = (t, "Eaten! Back to the start", "#ffffff")
                    score = 0
                    reset(false)
                    return
                }
            }
        }
        npc = npc.filter { !$0.dead && $0.x > -16 && $0.x < C + 16 }
        while npc.count < 14 { spawn(false) }
        e.setHud(["Score \(score)", "Size \(me.size) / 7", "Best \(Int(max(best ?? 0, Double(score))))"])
    }

    private func sign(_ v: Double) -> Double { v > 0 ? 1 : v < 0 ? -1 : 0 }

    override func draw(_ p: inout WaitingGamesPainter, _ t: Double) {
        p.bg(["#1d8aa0", "#0f5a70", "#062a38"])
        for i in 0..<5 {
            let fi = Double(i)
            let x = (fi * 0.23 + 0.05 + sin(t * 0.1 + fi) * 0.03) * C
            var y = 0.0
            while y < R * 0.75 {
                if (Int(y) + i) % 2 == 0 { p.txt(x + y * 0.35, y, "/", "#bff3ff", 0.06 * (1 - y / (R * 0.75))) }
                y += 1
            }
        }
        for b in bub {
            let y = jmod(jmod(b.y * R - t * b.v, R) + R, R)
            p.txt(b.x * C + sin(t * 2 + b.x * 9), y, "o", "#bdf2ff", 0.35)
        }
        for k in kelp {
            let h = k.h * R * 0.5
            var i = 0.0
            while i < h {
                p.txt(k.x * C + sin(i * 0.2 + t * 1.1 + k.p) * (i / h) * 2.5, R - 1 - i, Int(i) % 2 == 1 ? "(" : ")", "#3fae6a", 0.75)
                i += 1
            }
        }
        var x = 0.0
        while x < C { p.txt(x, R - 1, Int(x) % 3 != 0 ? "_" : ".", "#c9a96e", 0.6); x += 1 }
        for f in npc {
            let sh = fishShape(f.s).map { f.dir > 0 ? $0 : flip($0) }
            let big = f.s > me.size
            for (j, l) in sh.enumerated() { p.txt(f.x, f.y + Double(j), l, FISH_COL[f.s - 1], big ? 0.95 : 0.85) }
            if big && hypot((me.x - f.x) / 2, me.y - f.y) < 12 {
                p.txt(f.x + (f.dir > 0 ? Double(sh[0].count) : -2), f.y, "!", "#ff6b6b", 0.9)
            }
        }
        let sh = fishShape(me.size).map { me.dir > 0 ? $0 : flip($0) }
        let blink = me.inv > 0 && Int(floor(t * 8)) % 2 == 1
        if !blink {
            let w = Double(sh[min(1, sh.count - 1)].count)
            for (j, l) in sh.enumerated() { p.txt(me.x - w / 2, me.y - Double(sh.count) / 2 + Double(j), l, "#ffffff", 1) }
            p.txt(me.x - 1, me.y - Double(sh.count) / 2 - 1.2, "you", "#ffffff", 0.55)
        }
        if let f = flash, t - f.t < 1.4 { p.sans(W / 2, H * 0.28 - 16, f.txt, "#ffffff", 1 - (t - f.t) / 1.4, 20) }
    }
}

// ---------- Bot Race: type the words; your bots race you to the finish line.
private let WORDS = "deploy branch commit merge rebase revert stash cherry-pick checkout fetch push pull clone diff patch hotfix release tag fork agent token prompt context embedding model inference latency stream schema review cache socket widget kernel layout canvas thread signal module vector cursor sprite runtime shader lambda tensor router binary compiler linter bundler webpack vite docker container kubernetes cluster node pod ingress endpoint webhook payload header cookie session oauth jwt middleware handler callback promise async await closure iterator generator interface struct enum trait generic typescript javascript python rust golang swift kotlin react hooks state props reducer selector component render virtual effect suspense hydrate server client edge worker queue cron mutex deadlock thread-safe race-condition semaphore pipeline artifact build test mock stub fixture assert coverage regression flaky refactor legacy monorepo package dependency semver changelog migration index query join transaction rollback replica shard postgres redis sqlite graphql rest grpc json yaml toml regex unicode utf8 base64 hash checksum encrypt decrypt certificate localhost staging production rollout canary feature-flag config env secret vault terminal shell bash zsh vim tmux grep curl stdout stderr exit-code segfault null undefined nan overflow recursion stacktrace breakpoint debugger logger telemetry metrics heap stack garbage-collector pointer reference allocation buffer stream chunk throttle debounce memoize idempotent backoff retry".split(separator: " ").map(String.init)
private let RACER_COLOURS = ["#E07A5F", "#2dd4bf", "#60A5FA", "#A78BFA"]
private let GOAL = 15

private final class RaceGame: WaitingGame {
    struct Racer { let name: String; let col: String; let me: Bool; var pos = 0.0; var rate = 0.0; var jit = 0.0 }
    let lineup: [Racer]
    var racers: [Racer] = []
    var done: [Int] = []
    var state = "ready"
    var typed = ""
    var err = -10.0
    var words: [String] = []
    var idx = 0
    var t0 = 0.0
    var chars = 0

    init(_ e: WaitingGamesEngine, racers: [WaitingGameRacer]) {
        lineup = [Racer(name: "You", col: "#ffffff", me: true)]
            + racers.prefix(4).enumerated().map { i, r in Racer(name: r.name, col: r.color ?? RACER_COLOURS[i], me: false) }
        super.init(e)
    }

    override var hint: String { "Tap the game, then type. Each finished word moves your @ forward. Your bots race at their own pace." }
    override var keyboard: Bool { true }

    override func initGame() { newRace() }

    func newRace() {
        state = "ready"; done = []; typed = ""; err = -10
        words = (0..<GOAL).map { _ in WORDS.randomElement() ?? "deploy" }
        racers = lineup.map { r in var r = r; r.pos = 0; r.rate = r.me ? 0 : rnd(0.42, 0.62); r.jit = rnd(0, 9); return r }
        idx = 0; t0 = 0; chars = 0
        e.hideBanner()
        e.showBanner("Bot Race", "Type the word on screen. Each word moves you forward.\nStart typing to go.")
    }

    override func key(_ k: String) -> Bool {
        if state == "done" { if k == "Enter" { newRace() }; return false }
        if k.count != 1 && k != "Backspace" { return false }
        if state == "ready" { state = "go"; t0 = e.now; e.hideBanner() }
        let w = words[idx]
        if k == "Backspace" { if !typed.isEmpty { typed.removeLast() }; return true }
        if k == " " { return true }
        let next = typed + k.lowercased()
        if w.hasPrefix(next) {
            typed = next; chars += 1
            if next == w {
                idx += 1; typed = ""
                racers[0].pos = Double(idx) / Double(GOAL)
                if idx >= GOAL { finish(0) }
            }
        } else {
            err = e.now
        }
        return true
    }

    func finish(_ r: Int) {
        if !done.contains(r) { done.append(r) }
        guard racers[r].me || done.count == racers.count else { return }
        state = "done"
        let place = (done.firstIndex(of: 0) ?? -1) + 1
        let secs = e.now - t0
        let wpm = Int((Double(chars) / 5 / (secs / 60)).rounded())
        let ord = ["", "1st", "2nd", "3rd", "4th", "5th"]
        e.showBanner(place == 1 ? "You won!" : place > 0 ? "You came \(ord[place])" : "The bots won",
                     "\(wpm) words per minute · Press Enter to race again")
    }

    override func step(_ dt: Double, _ t: Double) {
        guard state == "go" else { return }
        for i in racers.indices where !racers[i].me && racers[i].pos < 1 {
            racers[i].pos = min(1, racers[i].pos + dt * racers[i].rate / Double(GOAL) * (1 + 0.35 * sin(t * 1.7 + racers[i].jit)))
            if racers[i].pos >= 1 { finish(i) }
        }
        let secs = e.now - t0
        e.setHud(["Word \(min(idx + 1, GOAL)) / \(GOAL)", "\(Int((Double(chars) / 5 / max(0.1, secs / 60)).rounded())) wpm"])
    }

    override func draw(_ p: inout WaitingGamesPainter, _ t: Double) {
        p.bg(["#16121f", "#221a33"])
        for i in 0..<60 {
            let fi = Double(i)
            p.txt(jmod(fi * 37, C), jmod(fi * 53, R), ".", "#c4b5fd", 0.15 + 0.15 * sin(t + fi))
        }
        let top = (R * 0.12).rounded(), laneH = max(2, floor(R * 0.11)), x0 = 12.0, x1 = C - 6, len = x1 - x0
        for (i, r) in racers.enumerated() {
            let y = top + Double(i) * laneH
            p.txt(1, y, String(r.name.padding(toLength: 8, withPad: " ", startingAt: 0).prefix(8)), r.col, 0.9)
            var x = x0
            while x < x1 { p.txt(x, y + 1, Int(x) % 2 == 1 ? "-" : "·", "#6b5b8a", 0.35); x += 1 }
            var k = 0.0
            while k < laneH - 1 { p.txt(x1, y + k, Int(k) % 2 == 1 ? "▒" : "█", "#ffffff", 0.5); k += 1 }
            let px = x0 + r.pos * len
            for k in 1..<6 { p.txt(px - Double(k), y, "=", "#ffffff", 0.18 * (1 - Double(k) / 6) * (state == "go" ? 1 : 0)) }
            let bob = state == "go" ? (sin(t * 10 + Double(i)) * 0.4).rounded() : 0
            p.txt(px, y + bob, r.me ? "@>" : "█>", r.col, 1)
            if let place = done.firstIndex(of: i) { p.txt(x1 + 2, y, "#\(place + 1)", r.col, 1) }
        }
        // the word to type
        let w = idx < words.count ? words[idx] : ""
        let wy = (R * 0.78).rounded()
        let size = e.FS * 2.4
        let big = WaitingGamesPainter.monoWidth(size, bold: true)
        let sx = W / 2 - Double(w.count) * big / 2
        let shaking = t - err < 0.25
        let shake = shaking ? sin(t * 90) * 4 : 0
        for (i, ch) in w.enumerated() {
            let col = i < typed.count ? "#F2A93B" : (shaking && i == typed.count ? "#f87171" : "#ece8ff")
            p.glyph(sx + Double(i) * big + shake, wy * chh, String(ch), col, size, bold: true)
        }
        let nx = words.dropFirst(idx + 1).prefix(3).joined(separator: "  ")
        let nw = Double(nx.count) * cw
        p.glyph(W / 2 - nw / 2, wy * chh + e.FS * 3.4, nx, "#ece8ff", e.FS, bold: false, 0.5)
    }
}

// ---------- Token Catch: catch the falling tokens, let the bugs fall.
private final class CatchGame: WaitingGame {
    struct Item { var x, y, v: Double; let bug: Bool; let ch: String; var gone = false }
    var items: [Item] = []
    var x = 0.0, tx = 0.0
    var score = 0, lives = 3, combo = 0
    var next = 0.0
    var over = false

    override var hint: String { "Move your finger to slide the catcher. Catch the green tokens. Let the red bugs fall past." }

    override func initGame() {
        items = []; x = C / 2; tx = C / 2; score = 0; lives = 3; next = 0; over = false; combo = 0; flash = nil
        e.hideBanner()
    }

    override func pointer(_ px: Double, _ py: Double) { tx = px / cw }
    override func click(_ px: Double, _ py: Double) { if over { initGame() } }

    override func step(_ dt: Double, _ t: Double) {
        if over { return }
        x += (tx - x) * min(1, dt * 12)
        next -= dt
        if next <= 0 {
            next = max(0.16, 0.5 - Double(score) * 0.006)
            let bug = Double.random(in: 0..<1) < 0.22
            let glyphs = Array("{}();<>=+")
            items.append(Item(x: floor(rnd(2, C - 3)), y: -1, v: rnd(7, 12) + Double(score) * 0.08, bug: bug,
                              ch: bug ? "ж" : String(glyphs[Int.random(in: 0..<9)])))
        }
        let py = playRows
        for i in items.indices {
            items[i].y += items[i].v * dt
            let it = items[i]
            if !it.gone && it.y >= py - 0.6 && it.y <= py + 0.8 && abs(it.x - x) <= 4 {
                items[i].gone = true
                if it.bug {
                    lives -= 1; combo = 0
                    flash = (t, "Bug caught. One life lost", "#f87171")
                    if lives <= 0 { over = true; e.showBanner("Out of lives", "\(score) tokens caught · Tap to play again") }
                } else {
                    score += 1; combo += 1
                    if combo % 10 == 0 { flash = (t, "\(combo) in a row", "#86efac") }
                }
            }
        }
        items = items.filter { !$0.gone && $0.y < R + 1 }
        e.setHud(["Tokens \(score)", String(repeating: "♥", count: max(0, lives)) + String(repeating: "♡", count: 3 - max(0, lives))])
    }

    override func draw(_ p: inout WaitingGamesPainter, _ t: Double) {
        p.bg(["#04130b", "#062015"])
        var c = 0
        while Double(c) < C {
            let off = jmod(t * Double(4 + (c * 7 % 5)) + Double(c * 13), R + 10)
            for k in 0..<6 {
                let code = 0x30a0 + ((c * 31 + k * 7 + Int(floor(t * 3))) % 90)
                if let scalar = Unicode.Scalar(UInt32(code)) {
                    p.txt(Double(c), floor(off) - Double(k), String(Character(scalar)), "#22c55e", 0.12 * (1 - Double(k) / 6))
                }
            }
            c += 3
        }
        for it in items { p.txt(it.x, it.y, it.ch, it.bug ? "#f87171" : "#86efac", 1) }
        p.txt(x.rounded() - 3, playRows, "[==A==]", "#ffffff", 1)
        flashDraw(p, t)
    }
}

// ---------- Meteor Dodge: fly through a meteor shower for as long as you can.
private final class DodgeGame: WaitingGame {
    struct Meteor { var x, y: Double; let vx, vy: Double; let s: Double }
    var m: [Meteor] = []
    var x = 0.0, y = 0.0, tx = 0.0, ty = 0.0, time = 0.0, next = 0.0
    var over = false

    override var hint: String { "Your finger flies the ship. Dodge the meteors. The shower gets thicker the longer you last." }

    override func initGame() {
        if best == nil { best = 0 }
        m = []; x = C / 2; y = R * 0.6; tx = x; ty = y; time = 0; over = false; next = 0
        e.hideBanner()
    }

    override func pointer(_ px: Double, _ py: Double) { tx = px / cw; ty = py / chh }
    override func click(_ px: Double, _ py: Double) { if over { initGame() } }

    override func step(_ dt: Double, _ t: Double) {
        if over { return }
        time += dt
        x += (tx - x) * min(1, dt * 10); y += (ty - y) * min(1, dt * 10)
        next -= dt
        if next <= 0 {
            next = max(0.1, 0.55 - time * 0.012)
            m.append(Meteor(x: rnd(C * 0.1, C * 1.3), y: -2, vx: -rnd(5, 10), vy: rnd(6, 11) + time * 0.15,
                            s: Double.random(in: 0..<1) < 0.25 ? 2 : 1))
        }
        for i in m.indices {
            m[i].x += m[i].vx * dt; m[i].y += m[i].vy * dt
            if hypot((m[i].x - x) * cw, (m[i].y - y) * chh) < m[i].s * 6 + 7 {
                over = true
                best = max(best ?? 0, time)
                e.showBanner("Hit!", String(format: "You lasted %.1f s · Best %.1f s · Tap to fly again", time, best ?? 0))
                break
            }
        }
        m = m.filter { $0.y < R + 3 && $0.x > -8 }
        e.setHud([String(format: "%.1f s", time), String(format: "Best %.1f s", max(best ?? 0, over ? 0 : time))])
    }

    override func draw(_ p: inout WaitingGamesPainter, _ t: Double) {
        p.bg(["#05060f", "#0d1030", "#1a1240"])
        for i in 0..<90 {
            let fi = Double(i)
            p.txt(jmod(fi * 97, C), jmod(fi * 61, R), i % 7 != 0 ? "." : "+", "#e0e7ff", 0.2 + 0.2 * sin(t * 2 + fi))
        }
        for mt in m {
            let n = hypot(mt.vx, mt.vy)
            for k in stride(from: 6, to: 0, by: -1) {
                let fk = Double(k)
                p.txt(mt.x - mt.vx / n * fk * 1.4, mt.y - mt.vy / n * fk * 0.7, k < 3 ? "*" : "·", "#fb923c", 0.7 * (1 - fk / 7))
            }
            p.txt(mt.x, mt.y, mt.s > 1 ? "@" : "o", "#fde68a", 1)
        }
        if !over || Int(floor(t * 6)) % 2 == 1 {
            p.txt(x - 1, y, "<A>", "#ffffff", 1)
            p.txt(x, y + 1, Int(floor(t * 12)) % 2 == 1 ? "'" : ".", "#7dd3fc", 0.8)
        }
    }
}

// ---------- Bug Bash: bugs pop out of your files for a moment; tap them before they hide.
private let FILES = ["api.ts", "App.tsx", "auth.rs", "schema.sql", "main.go", "index.css", "router.ts", "utils.py", "Dockerfile"]

private final class BugBashGame: WaitingGame {
    struct Hole { let f: String; var from = 0.0, until = 0.0; var gold = false; var hit = 0.0 }
    struct Splat { let x, y, t: Double; let txt: String }
    var holes: [Hole] = []
    var score = 0
    var left = 30.0
    var state = "ready"
    var next = 0.0
    var splat: [Splat] = []
    var clock = 0.0

    override var hint: String { "Bugs pop out of your files for a moment. Tap one before it hides. Gold bugs are worth 3. You have 30 seconds." }

    override func initGame() {
        if best == nil { best = 0 }
        holes = FILES.map { Hole(f: $0) }
        score = 0; left = 30; state = "ready"; next = 0; splat = []
        e.showBanner("Bug Bash", "Bugs pop out of your files. Tap them before they hide.\nTap to start. You have 30 seconds.")
    }

    func layout() -> (size: Double, gap: Double, ox: Double, oy: Double) {
        let gap = 14.0, top = 56.0, bottom = 84.0
        let size = min((W - gap * 4) / 3, (H - top - bottom - gap * 2) / 3)
        return (size, gap, (W - (size * 3 + gap * 2)) / 2, top + ((H - top - bottom) - (size * 3 + gap * 2)) / 2)
    }

    func rect(_ i: Int) -> (x: Double, y: Double, s: Double) {
        let L = layout()
        return (L.ox + Double(i % 3) * (L.size + L.gap), L.oy + Double(i / 3) * (L.size + L.gap), L.size)
    }

    override func click(_ px: Double, _ py: Double) {
        let t = clock
        if state != "go" { initGame(); state = "go"; e.hideBanner(); return }
        for i in holes.indices {
            let r = rect(i), h = holes[i]
            if px >= r.x && px <= r.x + r.s && py >= r.y && py <= r.y + r.s && h.until > t && h.hit == 0 {
                holes[i].hit = t; holes[i].until = t + 0.35
                let pts = h.gold ? 3 : 1
                score += pts
                splat.append(Splat(x: px, y: py, t: t, txt: "+\(pts)"))
            }
        }
    }

    override func step(_ dt: Double, _ t: Double) {
        clock = t
        guard state == "go" else { return }
        left -= dt
        if left <= 0 {
            left = 0; state = "done"
            best = max(best ?? 0, Double(score))
            e.showBanner("Time", "\(score) bugs squashed · Best \(Int(best ?? 0)) · Tap to play again")
            return
        }
        next -= dt
        if next <= 0 {
            let speed = 1 - (30 - left) / 30 * 0.45
            next = rnd(0.35, 0.7) * speed
            let free = holes.indices.filter { holes[$0].until < t }
            if let i = free.randomElement() {
                holes[i].from = t; holes[i].until = t + rnd(0.8, 1.2) * speed + 0.2
                holes[i].gold = Double.random(in: 0..<1) < 0.12; holes[i].hit = 0
            }
        }
        splat = splat.filter { t - $0.t < 0.7 }
        e.setHud(["Squashed \(score)", "\(Int(left.rounded(.up))) s", "Best \(Int(best ?? 0))"])
    }

    override func draw(_ p: inout WaitingGamesPainter, _ t: Double) {
        p.bg(["#0f1115", "#171a21"])
        for (i, h) in holes.enumerated() {
            let r = rect(i)
            p.roundRect(r.x, r.y, r.s, r.s, 14, Ink.color("#232733"))
            for k in 0..<5 {
                p.rect(r.x + 14, r.y + 16 + Double(k) * 14, (r.s - 28) * Double((k * 37 + i * 13) % 60 + 30) / 100, 5, Ink.color("#2f3545"))
            }
            p.ptxt(r.x + r.s / 2, r.y + r.s - 16, h.f, "#9aa3b5", 1, 12)
            if h.until > t {
                let rise = max(0, min(1, (t - h.from) / 0.12, (h.until - t) / 0.12))
                let fs = r.s * 0.16, cy = r.y + r.s * 0.45 + (1 - rise) * r.s * 0.3
                var clipped = p
                clipped.ctx.clip(to: Path(CGRect(x: r.x, y: r.y, width: r.s, height: r.s - 30)))
                let col = h.hit > 0 ? "#a3e635" : h.gold ? "#fde047" : "#f87171"
                let lines = h.hit > 0 ? ["\\ | /", "- * -", "/ | \\"] : [" \\ / ", "(•ж•)", " /|\\ "]
                for (j, l) in lines.enumerated() { clipped.ptxt(r.x + r.s / 2, cy + Double(j - 1) * fs * 1.05, l, col, rise, fs) }
            }
        }
        for s in splat { p.ptxt(s.x, s.y - 30 * (t - s.t), s.txt, "#a3e635", 1 - (t - s.t) / 0.7, 18) }
    }
}

// ---------- Stack Deploy: drop each release on the stack; only the part that lands stays.
private final class StackGame: WaitingGame {
    struct Block { var x, w: Double }
    struct Debris { var x, y, w, vy: Double; let c: Color }
    struct Mover { var x, lo, hi, w, dir, v: Double }
    let bh = 26.0
    var base = 0.0
    var blocks: [Block] = []
    var debris: [Debris] = []
    var mov = Mover(x: 0, lo: 0, hi: 0, w: 0, dir: 1, v: 150)
    var over = false
    var cam = 0.0
    var streak = 0
    var clock = 0.0

    override var hint: String { "Tap to drop the release. Only the part that lands on the stack stays. Line it up exactly for a Perfect; three in a row widens the stack again." }

    override func initGame() {
        if best == nil { best = 0 }
        base = H - 96
        let w = min(260, W * 0.5)
        blocks = [Block(x: (W - w) / 2, w: w)]
        debris = []; over = false; cam = 0; flash = nil; streak = 0
        e.hideBanner()
        spawn()
    }

    func spawn() {
        let top = blocks[blocks.count - 1], left = Double.random(in: 0..<1) < 0.5
        let span = top.w * 0.85
        mov = Mover(x: left ? top.x - span : top.x + span, lo: top.x - span, hi: top.x + span, w: top.w,
                    dir: left ? 1 : -1, v: 150 + Double(blocks.count) * 9)
    }

    func yOf(_ i: Int) -> Double { base - Double(i + 1) * bh + cam }

    func col(_ i: Int) -> Color {
        // hsl(h 70% 62%) as hue / saturation / brightness.
        Color(hue: Double((18 + i * 14) % 360) / 360, saturation: 0.6, brightness: 0.886)
    }

    func drop(_ t: Double) {
        if over { initGame(); return }
        let top = blocks[blocks.count - 1], m = mov
        var l = max(m.x, top.x), r = min(m.x + m.w, top.x + top.w)
        let n = blocks.count, y = yOf(n)
        if r - l < 8 {
            over = true
            debris.append(Debris(x: m.x, y: y, w: m.w, vy: 0, c: col(n)))
            best = max(best ?? 0, Double(n - 1))
            e.showBanner("It fell over", "\(n - 1) releases shipped · Best \(Int(best ?? 0)) · Tap to build again")
            return
        }
        if abs(m.x - top.x) < 6 {
            l = top.x; r = top.x + top.w
            streak += 1
            flash = (t, streak > 1 ? "Perfect ×\(streak)" : "Perfect!", "#fde68a")
            if streak > 2 { r = min(W * 0.6 + l, r + 8) }
        } else {
            streak = 0
            debris.append(Debris(x: m.x < top.x ? m.x : r, y: y, w: m.w - (r - l), vy: 0, c: col(n)))
        }
        blocks.append(Block(x: l, w: r - l))
        spawn()
    }

    override func resize() { base = H - 96 }
    override func click(_ px: Double, _ py: Double) { drop(clock) }
    override func key(_ k: String) -> Bool {
        if k == " " || k == "Enter" { drop(clock); return true }
        return false
    }

    override func step(_ dt: Double, _ t: Double) {
        clock = t
        let target = max(0, Double(blocks.count + 1) * bh - H * 0.5)
        cam += (target - cam) * min(1, dt * 5)
        if !over {
            mov.x += mov.dir * mov.v * dt
            if mov.dir < 0 && mov.x < mov.lo { mov.dir = 1 }
            if mov.dir > 0 && mov.x > mov.hi { mov.dir = -1 }
        }
        for i in debris.indices { debris[i].vy += 900 * dt; debris[i].y += debris[i].vy * dt }
        debris = debris.filter { $0.y < H + 60 }
        e.setHud(["Shipped \(blocks.count - 1)", "Best \(Int(max(best ?? 0, Double(blocks.count - 1))))"])
    }

    override func draw(_ p: inout WaitingGamesPainter, _ t: Double) {
        p.bg(["#14121c", "#1f1a2e"])
        for i in 0..<80 {
            let fi = Double(i)
            p.txt(jmod(fi * 47, C), jmod(fi * 71, R), "·", "#c4b5fd", 0.12 + 0.08 * sin(t + fi))
        }
        func blk(_ x: Double, _ y: Double, _ w: Double, _ c: Color, _ label: String, _ a: Double = 1) {
            p.rect(x, y, w, bh - 2, c.opacity(a))
            if !label.isEmpty && w > 48 {
                p.ctx.draw(Text(label).font(.system(size: 11, weight: .semibold, design: .monospaced)).foregroundColor(.black.opacity(0.42)),
                           at: CGPoint(x: x + 8, y: y + bh / 2 - 1), anchor: .leading)
            }
        }
        for (i, b) in blocks.enumerated() {
            let y = yOf(i)
            if y > -30 && y < H { blk(b.x, y, b.w, i > 0 ? col(i) : Ink.color("#3f3a52"), i > 0 ? "v0.\(i)" : "main") }
        }
        if !over { blk(mov.x, yOf(blocks.count), mov.w, col(blocks.count), "v0.\(blocks.count)") }
        for d in debris { blk(d.x, d.y, d.w, d.c, "", 0.7) }
        flashDraw(p, t)
    }
}

// ---------- Context Snake: your thread grows with every token. Fill the context window and the thread ends.
private final class SnakeGame: WaitingGame {
    struct Cell: Equatable { var x, y: Int }
    var gx = 0, gy = 0
    var s: [Cell] = []
    var d = Cell(x: 1, y: 0), nd = Cell(x: 1, y: 0)
    var acc = 0.0
    var over = false
    var score = 0
    var food = Cell(x: 0, y: 0)

    override var hint: String { "Tap the side you want to turn to (or use arrow keys). Each token makes the thread longer. Hit the edge or yourself and the context is full." }

    override func initGame() {
        gx = Int(floor(C / 2)); gy = Int(playRows)
        let sx = gx / 3, sy = gy / 2
        s = [Cell(x: sx, y: sy), Cell(x: sx - 1, y: sy), Cell(x: sx - 2, y: sy)]
        d = Cell(x: 1, y: 0); nd = d; acc = 0; over = false; score = 0
        food = place()
        e.hideBanner()
    }

    func place() -> Cell {
        guard gx > 2, gy > 3 else { return Cell(x: 1, y: 2) }
        for _ in 0..<500 {
            let f = Cell(x: Int(floor(rnd(1, Double(gx - 1)))), y: Int(floor(rnd(2, Double(gy - 1)))))
            if !s.contains(f) { return f }
        }
        return Cell(x: 1, y: 2)
    }

    override func key(_ k: String) -> Bool {
        if over && (k == "Enter" || k == " ") { initGame(); return true }
        let map: [String: (Int, Int)] = ["ArrowUp": (0, -1), "w": (0, -1), "ArrowDown": (0, 1), "s": (0, 1),
                                         "ArrowLeft": (-1, 0), "a": (-1, 0), "ArrowRight": (1, 0), "d": (1, 0)]
        guard let v = map[k] else { return false }
        turn(v.0, v.1)
        return true
    }

    func turn(_ x: Int, _ y: Int) {
        if x == -d.x && y == -d.y { return }
        nd = Cell(x: x, y: y)
    }

    override func click(_ px: Double, _ py: Double) {
        if over { initGame(); return }
        let h = s[0]
        let dx = px / cw / 2 - Double(h.x), dy = py / chh - Double(h.y)
        if d.x != 0 { turn(0, dy > 0 ? 1 : -1) } else { turn(dx > 0 ? 1 : -1, 0) }
    }

    override func step(_ dt: Double, _ t: Double) {
        if over { return }
        acc += dt
        let sp = max(0.055, 0.12 - Double(score) * 0.002)
        while acc > sp {
            acc -= sp
            d = nd
            let h = s[0], n = Cell(x: h.x + d.x, y: h.y + d.y)
            if n.x < 0 || n.y < 2 || n.x >= gx || n.y >= gy || s.contains(n) {
                over = true
                e.showBanner("Context window full", "\(s.count * 4)k tokens · Tap to start a new thread")
                return
            }
            s.insert(n, at: 0)
            if n == food { score += 1; food = place() } else { s.removeLast() }
        }
        e.setHud(["Context \(s.count * 4)k / 200k", "Tokens \(score)"])
    }

    override func draw(_ p: inout WaitingGamesPainter, _ t: Double) {
        p.bg(["#111318", "#1b1e26"])
        for x in 0..<max(0, gx) {
            p.txt(Double(x * 2), 1, "··", "#52525b", 0.6)
            p.txt(Double(x * 2), Double(gy), "··", "#52525b", 0.6)
        }
        for (i, c) in s.enumerated() {
            p.txt(Double(c.x * 2), Double(c.y), "██", i > 0 ? "#f0a58a" : "#D97757", i > 0 ? max(0.35, 1 - Double(i) / Double(s.count)) : 1)
        }
        p.txt(Double(food.x * 2), Double(food.y), "◆", "#F2A93B", 0.6 + 0.4 * sin(t * 6))
    }
}

// ---------- Bot Memory: flip cards and match your bots' faces.
private let FACES: [(String, String)] = [("(◕‿◕)", "#c2410c"), ("[•_•]", "#0f766e"), ("<o_o>", "#1d4ed8"), ("{^_^}", "#6d28d9"),
                                         ("(⌐■_■)", "#b45309"), ("[@_@]", "#be185d"), ("(-_-)", "#475569"), ("\\ò_ó/", "#15803d")]

private final class MemoryGame: WaitingGame {
    struct Card { let f: Int; var up = false; var done = false; var flip = 0.0 }
    var cards: [Card] = []
    var open: [Int] = []
    var moves = 0
    var lock = 0.0
    var t0 = 0.0
    var won = false
    var clock = 0.0

    override var hint: String { "Tap a card to flip it, then find its pair. Match all eight bot faces in as few moves as you can." }

    override func initGame() {
        cards = (0..<16).map { $0 >> 1 }.shuffled().map { Card(f: $0) }
        open = []; moves = 0; lock = 0; t0 = 0; won = false
        e.hideBanner()
    }

    func layout() -> (cwid: Double, ch: Double, gap: Double, ox: Double, oy: Double) {
        let gap = 10.0, top = 50.0, bottom = 70.0
        let cwid = min((W - gap * 5) / 4, ((H - top - bottom - gap * 3) / 4) * 1.3), ch = cwid / 1.3
        return (cwid, ch, gap, (W - (cwid * 4 + gap * 3)) / 2, top + ((H - top - bottom) - (ch * 4 + gap * 3)) / 2)
    }

    override func click(_ px: Double, _ py: Double) {
        if won { initGame(); return }
        if lock > 0 { return }
        let L = layout()
        let c = Int(floor((px - L.ox) / (L.cwid + L.gap))), r = Int(floor((py - L.oy) / (L.ch + L.gap)))
        if c < 0 || c > 3 || r < 0 || r > 3 { return }
        let i = r * 4 + c
        if cards[i].up || cards[i].done { return }
        if t0 == 0 { t0 = clock }
        cards[i].up = true
        open.append(i)
        if open.count == 2 {
            moves += 1
            let a = open[0], b = open[1]
            if cards[a].f == cards[b].f {
                cards[a].done = true; cards[b].done = true
                open = []
                if cards.allSatisfy(\.done) {
                    won = true
                    e.showBanner("All matched", "\(moves) moves · \(Int((clock - t0).rounded())) s · Tap to deal again")
                }
            } else {
                lock = clock + 0.8
            }
        }
    }

    override func step(_ dt: Double, _ t: Double) {
        clock = t
        if lock > 0 && t > lock {
            for i in open { cards[i].up = false }
            open = []; lock = 0
        }
        for i in cards.indices { cards[i].flip += ((cards[i].up || cards[i].done ? 1 : 0) - cards[i].flip) * min(1, dt * 12) }
        e.setHud(["Moves \(moves)", "Pairs \(cards.filter(\.done).count / 2) / 8"])
    }

    override func draw(_ p: inout WaitingGamesPainter, _ t: Double) {
        p.bg(["#1a1625", "#251d33"])
        for i in 0..<70 {
            let fi = Double(i)
            p.txt(jmod(fi * 41, C), jmod(fi * 67, R), "·", "#c4b5fd", 0.15 + 0.1 * sin(t + fi))
        }
        let L = layout()
        for (i, k) in cards.enumerated() {
            let x = L.ox + Double(i % 4) * (L.cwid + L.gap), y = L.oy + Double(i >> 2) * (L.ch + L.gap)
            var c = p.ctx
            c.translateBy(x: x + L.cwid / 2, y: y + L.ch / 2)
            c.scaleBy(x: max(0.04, abs(1 - 2 * k.flip)), y: 1)
            c.opacity = k.done ? 0.6 : 1
            let face = k.flip > 0.5
            c.fill(Path(roundedRect: CGRect(x: -L.cwid / 2, y: -L.ch / 2, width: L.cwid, height: L.ch), cornerRadius: 10),
                   with: .color(Ink.color(face ? "#f4f1fb" : "#3b2f55")))
            if face {
                c.draw(Text(FACES[k.f].0).font(.system(size: (L.ch * 0.24).rounded(), weight: .semibold, design: .monospaced))
                    .foregroundColor(Ink.color(FACES[k.f].1)), at: .zero, anchor: .center)
            } else {
                c.draw(Text("A://").font(.system(size: (L.ch * 0.16).rounded(), weight: .semibold, design: .monospaced))
                    .foregroundColor(Ink.color("#8b7bb0")), at: .zero, anchor: .center)
            }
        }
    }
}
