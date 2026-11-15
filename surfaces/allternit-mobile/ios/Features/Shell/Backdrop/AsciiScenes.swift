import Foundation

/// The twenty ASCII looks (allternit-ai ascii/ascii-scenes.ts): one Allternit
/// look (the A-block mark), six moods and thirteen scenes. Each `make` sets
/// up a look for a grid of c × r cells and returns the per-frame draw.
enum AsciiScenes {
    private typealias G = AsciiGrid
    private static let tau = Ascii.tau
    private static func ch(_ s: String, _ i: Int) -> Character { s[s.index(s.startIndex, offsetBy: max(0, min(s.count - 1, i)))] }

    static let aBlock = AsciiScene(
        id: "ascii-a-block", name: "A-block",
        note: "The A-block mark breathes, sweeps, runs and writes; it leans away from your finger",
        bg: ["#111214", "#1b1c20"], tile: true, solid: true, own: true
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        // x, y on the 5 × 5 grid, outline order, stroke order.
        let raw: [[Double]] = [[2, 0, 4, 4], [1, 1, 3, 3], [3, 1, 5, 5], [0, 2, 2, 2], [1, 2, 10, 9], [3, 2, 9, 10], [4, 2, 6, 6], [0, 3, 1, 1], [4, 3, 7, 7], [0, 4, 0, 0], [4, 4, 8, 8]]
        let B = raw.map { (x: $0[0], y: $0[1], ol: Int($0[2]), st: $0[3]) }
        let slots = B.sorted { $0.ol < $1.ol }.map { [$0.x, $0.y] }
        let U = max(1.2, (rd * 0.54) / 5)
        let UC = U * 2
        let bh = max(1, Int((U * 0.82).rounded()))
        let bw = max(2, Int((UC * 0.82).rounded()))
        let ox = cd / 2 - 2.5 * UC
        let oy = rd * 0.48 - 2.5 * U
        let scatter = B.map { _ in [(R() - 0.5) * 2, (R() - 0.5) * 2] }
        let parts = (0..<Ascii.count(cd * rd / 80, D, 6)).map { _ in (a: R() * tau, d: 0.35 + R() * 0.65, v: 0.05 + R() * 0.08, p: R(), coral: R() < 0.25) }
        let INK = "#d8d4cc", CORE = "#D97757"
        func block(_ g: G, _ gx: Double, _ gy: Double, _ col: String) {
            let x0 = (ox + gx * UC + (UC - Double(bw)) / 2).rounded()
            let y0 = (oy + gy * U + (U - Double(bh)) / 2).rounded()
            for j in 0..<bh { for i in 0..<bw { g.put(x0 + Double(i), y0 + Double(j), " ", col, 1) } }
        }
        return { g, t, P in
            for gy in -12..<16 {
                for gx in -24..<28 {
                    let x = (ox + Double(gx) * UC + UC / 2).rounded(), y = (oy + Double(gy) * U + U / 2).rounded()
                    if x < 0 || x >= cd || y < 0 || y >= rd { continue }
                    var al = 0.16
                    if P.s > 0.01 { al += max(0, 1 - hypot(x - P.x, (y - P.y) * 2) / 18) * 0.5 * P.s }
                    g.put(x, y, "·", "#8a8f98", al)
                }
            }
            let ccx = ox + 2.5 * UC, ccy = oy + 2.5 * U
            let reach = hypot(cd / 2, rd)
            for q in parts {
                let life = Ascii.mod(q.p + t * q.v, 1)
                let d = (1 - life) * reach * q.d
                g.put(ccx + cos(q.a) * d, ccy + sin(q.a) * d * 0.5, "▪", q.coral ? CORE : INK, 0.2 + 0.45 * life)
            }
            // Breathe, sweep, conveyor, write: five seconds each.
            let ph = Ascii.mod(t, 20) / 5
            let st = Int(ph)
            let u = ph - Double(st)
            for (i, b) in B.enumerated() {
                var gx = b.x, gy = b.y
                var col = INK
                var show = true
                if st == 0 {
                    let k = pow(sin(u * .pi), 2) * 0.35
                    gx += (b.x - 2) * k
                    gy += (b.y - 2) * k
                } else if st == 1 {
                    let sw = Ascii.mod(atan2(b.x - 2, 2 - b.y) / tau + 1, 1)
                    let s2 = Ascii.mod(u * 2, 1)
                    let dd = min(abs(sw - s2), 1 - abs(sw - s2))
                    let k = max(0, 1 - dd * 7)
                    gx += (b.x - 2) * k * 0.28
                    gy += (b.y - 2) * k * 0.28
                    if k > 0.35 { col = "#ffffff" }
                } else if st == 2 {
                    let pp = u * 11
                    let sI = Int(pp)
                    let f = Ascii.smooth(pp - Double(sI))
                    let fr = slots[(b.ol + sI) % 11], to = slots[(b.ol + sI + 1) % 11]
                    gx = fr[0] + (to[0] - fr[0]) * f
                    gy = fr[1] + (to[1] - fr[1]) * f
                } else {
                    let k = u < 0.2 ? 1 - Ascii.smooth(u / 0.2) : min(1, max(0, (u - 0.2) * 16 - b.st))
                    show = k > 0.02
                    gx = 2 + (b.x - 2) * Ascii.smooth(k)
                    gy = 2 + (b.y - 2) * Ascii.smooth(k)
                }
                if P.s > 0.01 {
                    let dx = gx - ((P.x - ox) / UC - 0.5)
                    let dy = gy - ((P.y - oy) / U - 0.5)
                    let d = hypot(dx, dy) + 0.01
                    let f = P.s * pow(max(0, 1 - d / 3.4), 2) * 1.3
                    gx += dx / d * f
                    gy += dy / d * f
                }
                let age = t - P.wt
                if age >= 0 && age < 2.4 {
                    let e = age < 0.3 ? Ascii.smooth(age / 0.3) : max(0, 1 - Ascii.smooth((age - 0.3) / 2.1))
                    gx += scatter[i][0] * 5 * e
                    gy += scatter[i][1] * 3.5 * e
                }
                if show { block(g, gx, gy, col) }
            }
            var cx2 = 2.0, cy2 = 2.0
            if P.s > 0.01 {
                cx2 += max(-0.7, min(0.7, ((P.x - ox) / UC - 2.5) * 0.15)) * P.s
                cy2 += max(-0.7, min(0.7, ((P.y - oy) / U - 2.5) * 0.15)) * P.s
            }
            let hx = ox + (cx2 + 0.5) * UC, hy = oy + (cy2 + 0.5) * U
            let rr = U * 1.1 + sin(t * 2) * 0.3
            var a = 0.0
            while a < tau {
                g.put(hx + cos(a) * rr * 2, hy + sin(a) * rr, "·", CORE, 0.35)
                a += 0.18
            }
            block(g, cx2, cy2, st == 2 && Int(u * 11) % 2 == 1 ? "#e8906f" : CORE)
        }
    }

    static let clay = AsciiScene(
        id: "ascii-clay", name: "Clay Contours",
        note: "Warm paper with terracotta contour lines; calm and bookish",
        bg: ["#f7f1e7", "#ede3d3"]
    ) { c, r, R, D in
        let sd = R() * 100
        let sc = 0.05 / max(0.6, min(1.4, Double(c) / 90))
        var V = [Double](repeating: 0, count: c * r)
        return { g, t, _ in
            let k = 7 + 3 * D
            for y in 0..<r { for x in 0..<c { V[y * c + x] = Ascii.fbm(Double(x) * sc + sd, Double(y) * sc * 2 + t * 0.035, 7) * k } }
            for y in 0..<r {
                for x in 0..<c {
                    let b = Int(V[y * c + x].rounded(.down))
                    let bx = x + 1 < c ? Int(V[y * c + x + 1].rounded(.down)) : b
                    let by = y + 1 < r ? Int(V[(y + 1) * c + x].rounded(.down)) : b
                    if bx != b || by != b {
                        let hi = max(b, bx, by)
                        let glyph: Character = bx != b && by != b ? ((bx > b) == (by > b) ? "/" : "\\") : by != b ? "-" : "|"
                        g.put(Double(x), Double(y), hi % 4 == 0 ? "~" : glyph, hi % 2 != 0 ? "#c2603e" : "#8a7a66", hi % 4 == 0 ? 0.95 : 0.6)
                    } else if (x * 7 + y * 13) % 23 == 0 {
                        g.put(Double(x), Double(y), ".", "#b8a68e", 0.3 * D)
                    }
                }
            }
        }
    }

    static let inkBloom = AsciiScene(
        id: "ascii-ink-bloom", name: "Ink Bloom",
        note: "A soft black-and-white cloud breathes in the middle",
        bg: ["#fcfcfc", "#eeeeee"]
    ) { c, r, _, D in
        let RAMP = " .·:-=+*#%@"
        let cd = Double(c), rd = Double(r)
        return { g, t, _ in
            let fx = cd / 2, fy = rd / 2
            let rad = min(cd / 2.1, rd) * 0.44 * (1 + 0.07 * sin(t * 0.8))
            for y in 0..<r {
                for x in 0..<c {
                    let dx = (Double(x) - fx) / 2, dy = Double(y) - fy
                    let d = hypot(dx, dy) / rad
                    if d > 1.3 { continue }
                    let a = atan2(dy, dx)
                    let n = Ascii.fbm(cos(a) * 1.6 + t * 0.18 + dx * 0.05, sin(a) * 1.6 - t * 0.14 + dy * 0.05, 3)
                    let v = max(0, (1 - d) * 1.25 + (n - 0.5) * (0.7 + 0.4 * D))
                    let i = min(RAMP.count - 1, Int(v * Double(RAMP.count)))
                    if i > 0 { g.put(Double(x), Double(y), ch(RAMP, i), "#161616", 0.22 + 0.68 * min(1, v)) }
                }
            }
        }
    }

    static let spectrum = AsciiScene(
        id: "ascii-spectrum", name: "Spectrum",
        note: "Blue to violet to rose ribbons, with sparks that twinkle",
        bg: ["#0a0f22", "#151a38"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let GC = ["#4f8df5", "#7b6cf0", "#a86ad8", "#d9668a"]
        func gc(_ x: Double) -> String { GC[max(0, min(3, Int((x / cd) * 4)))] }
        let S = (0..<Ascii.count(cd * rd / 40, D, 8)).map { _ in (x: (R() * cd).rounded(.down), y: (R() * rd).rounded(.down), p: R() * tau, s: 0.6 + R() * 1.6) }
        return { g, t, _ in
            for k in 0..<4 {
                let kd = Double(k)
                for xi in 0..<c {
                    let x = Double(xi)
                    let y = rd * (0.28 + kd * 0.14) + sin(x * 0.045 + t * (0.35 + kd * 0.07) + kd * 1.3) * rd * 0.12 + sin(x * 0.013 - t * 0.2) * rd * 0.05
                    let a = 0.5 + 0.5 * (0.5 + 0.5 * sin(x * 0.08 - t + kd))
                    g.put(x, y, "≈", gc(x), a)
                    g.put(x, y + 1, "~", gc(x), a * 0.6)
                    g.put(x, y - 1, "-", gc(x), a * 0.4)
                    g.put(x, y + 2, ".", gc(x), a * 0.3)
                }
            }
            for s in S {
                let v = sin(t * s.s + s.p)
                if v <= 0.2 { continue }
                let col = gc(s.x)
                g.put(s.x, s.y, v > 0.85 ? "*" : "+", col, 0.4 + 0.6 * v)
                if v > 0.92 {
                    g.put(s.x - 1, s.y, "-", col, 0.55)
                    g.put(s.x + 1, s.y, "-", col, 0.55)
                    g.put(s.x, s.y - 1, "|", col, 0.45)
                    g.put(s.x, s.y + 1, "|", col, 0.45)
                }
            }
        }
    }

    static let eventHorizon = AsciiScene(
        id: "ascii-event-horizon", name: "Event Horizon",
        note: "A black hole in black and white; its disk turns slowly",
        bg: ["#000000", "#0b0b0b"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let S = Ascii.stars(R, c, rd, (cd * rd / 35) * D)
        let RAMP = ".:-=+*#"
        return { g, t, _ in
            Ascii.drawStars(g, S, t, "#ffffff")
            let cx = cd / 2, cy = rd / 2
            let K = min(cd / 2.3, rd * 0.95) / 12
            for y in 0..<r {
                for x in 0..<c {
                    let dx = (Double(x) - cx) / 2 / K, dy = (Double(y) - cy) / K
                    let d = hypot(dx, dy)
                    if d < 2.5 { g.erase(Double(x), Double(y)); continue }
                    let ey = dy / 0.3
                    let de = hypot(dx, ey)
                    var v = 0.0
                    if de > 3 && de < 9.5 { v = (1 - abs(de - 5) / 4.5) * (0.5 + 0.5 * sin(atan2(ey, dx) * 3 - t * 1.1 + de * 0.9)) }
                    if dy < 0 && de < 9.5 && de > 3 { v *= 0.55 }
                    if d < 3.3 { v = max(v, 1 - abs(d - 2.85) * 2.4) }
                    if v > 0.08 { g.put(Double(x), Double(y), ch(RAMP, min(6, Int(v * 7))), "#f4f4f4", 0.2 + 0.8 * v) }
                }
            }
        }
    }

    static let lattice = AsciiScene(
        id: "ascii-lattice", name: "Lattice",
        note: "A teal lattice with pulses running along its lines",
        bg: ["#0b1416", "#10211f"]
    ) { c, r, R, D in
        let P = 8
        let span = Int(((Double(c) / 2 + Double(r)) / Double(P)).rounded(.up))
        let pulses = (0..<Ascii.count(10, D, 3)).map { _ in (k: Int(R() * Double(span)), o: R() * 50, v: 5 + R() * 8, dir: R() < 0.5 ? 1 : -1) }
        return { g, t, _ in
            for y in 0..<r {
                var x = 0
                while x < c {
                    let u = ((x / 2 + y) % P + P) % P
                    let w = ((x / 2 - y) % P + P) % P
                    if u == 0 && w == 0 { g.put(Double(x), Double(y), "o", "#5fd3d0", 0.75) }
                    else if u == 0 { g.put(Double(x), Double(y), "/", "#2c7a80", 0.4) }
                    else if w == 0 { g.put(Double(x), Double(y), "\\", "#2c7a80", 0.4) }
                    x += 2
                }
            }
            let span = Double(r + 8)
            for p in pulses {
                for i in 0..<6 {
                    let yy = (Ascii.wrap(p.o + t * p.v - Double(i) * 0.9, span) - 4).rounded()
                    let x = p.dir > 0 ? 2 * (Double(p.k * P) - yy) : 2 * (Double((p.k - r / P) * P) + yy)
                    g.put(x, yy, p.dir > 0 ? "/" : "\\", "#9ff5ef", i > 0 ? 0.75 * (1 - Double(i) / 6) : 1)
                }
            }
        }
    }

    static let ember = AsciiScene(
        id: "ascii-ember", name: "Ember Blocks",
        note: "Stacks of warm pixel blocks rise and fall, red through yellow",
        bg: ["#15100c", "#221812"], tile: true
    ) { c, r, R, D in
        let GC = ["#ffd800", "#ffaf00", "#ff8205", "#fa500f", "#e10500"]
        let w = 3
        let rd = Double(r)
        let ph = (0..<Int((Double(c) / Double(w)).rounded(.up))).map { _ in R() * tau }
        return { g, t, _ in
            for i in ph.indices {
                let id = Double(i)
                let h = Int((rd * (0.12 + 0.4 * D * 0.6 * (0.5 + 0.5 * sin(t * 0.9 + ph[i])) * (0.6 + 0.4 * sin(id * 0.35 + t * 0.3)))).rounded())
                if h > 0 {
                    for k in 0..<h {
                        let col = GC[max(0, min(4, Int((1 - Double(k) / max(1, rd * 0.55)) * 5)))]
                        for j in 0..<(w - 1) { g.put(Double(i * w + j), rd - 1 - Double(k), " ", col, 0.95) }
                    }
                    g.put(Double(i * w), rd - 2 - Double(h) - (1 + abs(sin(t * 2 + ph[i])) * 2).rounded(), " ", "#ffd800", 0.85)
                }
            }
        }
    }

    static let skyDrift = AsciiScene(
        id: "ascii-sky-drift", name: "Sky Drift",
        note: "Puffy clouds drift across a soft blue sky at two depths",
        bg: ["#9fd2ff", "#e8f4ff"], tile: true
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let far = (0..<Ascii.count(cd / 12, D)).map { _ in (x: R() * cd, y: 1 + R() * rd * 0.55, s: 0.25 + R() * 0.3, w: Int(6 + R() * 6)) }
        let near = (0..<Ascii.count(cd / 14, D, 3)).map { _ in (x: R() * cd, y: rd * 0.18 + R() * rd * 0.7, s: 0.8 + R() * 0.6, w: Int(10 + R() * 12), p: R() * 9) }
        return { g, t, _ in
            for k in far {
                let x = Ascii.mod(k.x + t * k.s, cd + Double(k.w)) - Double(k.w) / 2
                for i in 0..<k.w { g.put(x + Double(i), k.y, i % 2 == 1 ? "." : "-", "#ffffff", 0.55) }
            }
            for k in near {
                let x0 = Ascii.mod(k.x + t * k.s, cd + Double(k.w * 2)) - Double(k.w)
                let h = max(2, k.w / 5)
                for y in -h...1 {
                    for i in 0..<k.w {
                        let u = (Double(i) / Double(k.w - 1)) * 2 - 1
                        if Double(y) >= -Double(h) * (1 - u * u) * (0.75 + 0.25 * sin(Double(i) * 1.3 + k.p)) {
                            g.put(x0 + Double(i), k.y + Double(y), y == 1 ? "_" : ":", y == 1 ? "#d3e5f6" : "#ffffff", 0.95)
                        }
                    }
                }
            }
        }
    }

    static let glyphStars = AsciiScene(
        id: "ascii-glyph-stars", name: "Glyph Stars",
        note: "A:// glyphs drift and link up",
        bg: ["#0a0f24", "#1b2143"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let n = Ascii.count(cd * rd / 60, D, 10)
        let glyphs = "A:/*+"
        let s = (0..<n).map { _ in (x: R() * cd, y: R() * rd, vx: (R() - 0.5) * 0.5, vy: (R() - 0.5) * 0.25, ch: ch(glyphs, Int(R() * 5)), p: R() * tau, big: R() < 0.2) }
        func slope(_ dx: Double, _ dy: Double) -> Character {
            let q = Ascii.wrap(atan2(dy * 2, dx), .pi)
            return q < 0.4 || q > 2.75 ? "-" : q < 1.2 ? "\\" : q < 1.95 ? "|" : "/"
        }
        return { g, t, _ in
            let pos = s.map { (x: Ascii.wrap($0.x + $0.vx * t, cd), y: Ascii.wrap($0.y + $0.vy * t, rd)) }
            for i in 0..<n {
                for j in (i + 1)..<max(i + 1, n) {
                    let A = pos[i], B = pos[j]
                    let dx = B.x - A.x, dy = B.y - A.y
                    let d = hypot(dx / 2, dy)
                    if d >= 7 { continue }
                    let al = 0.6 * (1 - d / 7)
                    Ascii.line(g, A.x, A.y, B.x, B.y, slope(dx, dy), "#7f8fff", al)
                    let f = Ascii.mod(t * 0.5 + Double(i) * 0.37 + Double(j) * 0.11, 1)
                    g.put(A.x + dx * f, A.y + dy * f, "•", "#e3e8ff", min(1, al * 1.8))
                }
            }
            for (i, p) in s.enumerated() {
                let tw = 0.6 + 0.4 * sin(t * 2 + p.p)
                if p.big {
                    for o in [(-2.0, 0.0), (2, 0), (0, -1), (0, 1)] { g.put(pos[i].x + o.0, pos[i].y + o.1, "·", "#aab6ff", 0.4 * tw) }
                }
                g.put(pos[i].x, pos[i].y, p.ch, p.big ? "#ffffff" : "#d6dcff", tw)
            }
        }
    }

    static let rainGlass = AsciiScene(
        id: "ascii-rain-glass", name: "Rain on Glass",
        note: "Streaks fall and drops slide down the window, city lights behind",
        bg: ["#1c2836", "#4a5e72"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let drops = (0..<Ascii.count(cd * 0.7, D, 8)).map { _ in (x: (R() * cd).rounded(.down), y: R() * rd, v: 10 + R() * 12, l: Int(3 + R() * 5)) }
        let beads = (0..<Ascii.count(cd * rd / 70, D, 6)).map { _ in (x: (R() * cd).rounded(.down), y: R() * rd, v: R() < 0.35 ? 0.4 + R() * 1.2 : 0) }
        let palette = ["#fbbf24", "#f472b6", "#60a5fa"]
        let lights = (0..<Ascii.count(cd / 8, D, 3)).map { _ in (x: R() * cd, y: rd * (0.3 + R() * 0.6), col: palette[Int(R() * 3)], p: R() * tau) }
        return { g, t, _ in
            for l in lights {
                for i in -1...1 { for j in -1...1 { g.put(l.x + Double(i * 2), l.y + Double(j), "o", l.col, 0.18 + 0.08 * sin(t + l.p)) } }
            }
            for k in drops {
                let y = Ascii.mod(k.y + t * k.v, rd + Double(k.l))
                for i in 0..<k.l { g.put(k.x, y - Double(i), i == 0 ? "|" : i < 3 ? ":" : ".", "#d4e4f4", (1 - Double(i) / Double(k.l)) * 0.6) }
            }
            for k in beads {
                let y = Ascii.mod(k.y + t * k.v, rd)
                g.put(k.x, y, "o", "#eef6ff", 0.85)
                if k.v > 0 {
                    g.put(k.x, y - 1, ":", "#eef6ff", 0.35)
                    g.put(k.x, y - 2, ".", "#eef6ff", 0.2)
                }
            }
        }
    }

    static let codeRain = AsciiScene(
        id: "ascii-code-rain", name: "Code Rain",
        note: "Soft streams of code characters, made for Code mode",
        bg: ["#05110d", "#0a241d"]
    ) { c, r, R, D in
        let CH = "const=>{}();01aAfn<>/*[]$#"
        let rd = Double(r)
        var s: [(x: Double, y: Double, v: Double, l: Int)] = []
        for x in 0..<c where R() < 0.45 * D { s.append((Double(x), R() * rd * 2, 4 + R() * 9, Int(6 + R() * 14))) }
        return { g, t, _ in
            for k in s {
                let y = Ascii.mod(k.y + t * k.v, rd + Double(k.l))
                for i in 0..<k.l {
                    let yy = y.rounded(.down) - Double(i)
                    let glyph = ch(CH, Int(Ascii.hash(k.x * 131 + yy * 17 + (t * 3 + Double(i)).rounded(.down)) * Double(CH.count)))
                    g.put(k.x, yy, glyph, i > 0 ? "#4fd1a5" : "#d8fff2", i > 0 ? 0.75 * (1 - Double(i) / Double(k.l)) : 1)
                }
            }
        }
    }

    static let northernLights = AsciiScene(
        id: "ascii-northern-lights", name: "Northern Lights",
        note: "Curtains of light over a still lake that mirrors them",
        bg: ["#030712", "#0b1a30"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let S = Ascii.stars(R, c, rd, (cd * rd / 40) * D, top: 0.5)
        let lake = Int((rd * 0.72).rounded())
        let p1 = R() * tau
        let h = (0..<c).map { x in Int((1 + abs(sin(Double(x) * 0.06 + p1)) * rd * 0.08 + Ascii.fbm(Double(x) * 0.15, 1, 4) * rd * 0.06).rounded()) }
        let C = ["#34d399", "#5eead4", "#818cf8", "#c084fc"]
        return { g, t, _ in
            Ascii.drawStars(g, S, t, "#cbd5ff")
            for x in 0..<c {
                let xd = Double(x)
                let top = rd * 0.06 + Ascii.fbm(xd * 0.04 - t * 0.08, t * 0.05, 5) * rd * 0.25
                let L = rd * (0.15 + 0.35 * Ascii.fbm(xd * 0.09 + t * 0.12, 3, 6)) * (0.7 + 0.3 * D)
                let sh = 0.35 + 0.65 * Ascii.fbm(xd * 0.12 - t * 0.3, 4, 8)
                let cc = C[min(3, Int(Ascii.fbm(xd * 0.03 + t * 0.05, 2, 9) * 4.4))]
                var i = 0.0
                while i < L {
                    let y = top + i
                    if y >= Double(lake - h[x]) { break }
                    let f = i / L
                    g.put(xd, y, i < 1 ? "≈" : f < 0.35 ? "|" : f < 0.7 ? ":" : ".", cc, sh * (i < 1 ? 0.9 : (1 - f) * 0.95))
                    i += 1
                }
                for y in (lake - h[x])..<max(lake - h[x], lake) { g.put(xd, Double(y), y == lake - h[x] ? "^" : "#", "#1c2740", 1) }
            }
            for y in lake..<r {
                let my = 2 * lake - 1 - y
                if my < 0 { continue }
                for x in 0..<c {
                    let sx = Int((Double(x) + sin(Double(y) * 0.9 + t * 1.5) * 0.8).rounded())
                    if sx < 0 || sx >= c { continue }
                    if let cell = g.cell(my * c + sx) { g.put(Double(x), Double(y), cell.ch == "|" ? ":" : cell.ch, cell.col, cell.a * 0.32) }
                }
            }
        }
    }

    static let tide = AsciiScene(
        id: "ascii-tide", name: "Tide",
        note: "Sunset over slow waves; birds and a small sailboat pass",
        bg: ["#ffb37a", "#ffd6a8", "#6fb9cf"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let hz = Int((rd * 0.55).rounded())
        let hzd = Double(hz)
        let sx = cd * 0.68
        let sr = max(3, rd * 0.13)
        let birds = (0..<Ascii.count(4, D, 2)).map { _ in (o: R() * cd, y: rd * (0.1 + R() * 0.25), v: 2 + R() * 2) }
        return { g, t, _ in
            for y in (hz - Int(sr.rounded(.up)))..<hz {
                var x = (sx - sr * 2.2).rounded(.down)
                while x <= sx + sr * 2.2 {
                    let dx = (x - sx) / 2.1, dy = Double(y) - (hzd - 1)
                    if dx * dx + dy * dy < sr * sr { g.put(x, Double(y), "@", Double(y) < hzd - sr * 0.5 ? "#ff9f5a" : "#ff6f3c", 0.92) }
                    x += 1
                }
            }
            for i in 0..<3 {
                let id = Double(i)
                let y = hzd - sr - 3 - id * 2.5
                let w = 10 + i * 4
                let x = Ascii.mod(id * 37 + t * (1 + id * 0.4), cd + Double(w)) - Double(w)
                for j in 0..<w { g.put(x + Double(j), y, "=", "#fff3e6", 0.7) }
            }
            for b in birds { g.put(Ascii.mod(b.o + t * b.v, cd + 4) - 2, b.y + sin(t * 3 + b.o) * 0.4, sin(t * 8 + b.o) > 0 ? "v" : "-", "#5a3a3a", 0.8) }
            for y in hz..<r {
                let f = (Double(y) - hzd) / (rd - hzd)
                for x in 0..<c {
                    let xd = Double(x), yd = Double(y)
                    let v = sin(xd * 0.22 - t * 1.4 + yd * 0.9) + sin(xd * 0.09 + t * 0.6 - yd * 0.3)
                    let glyph: Character? = v > 1.1 ? "~" : v > 0.2 ? "-" : v > -0.7 ? "_" : nil
                    if let glyph { g.put(xd, yd, glyph, "#1f6f8c", 0.35 + f * 0.55) }
                }
                let rw = sr * 2.2 * (1 - f * 0.7)
                var x = (sx - rw).rounded(.down)
                while x < sx + rw {
                    if Ascii.hash(x * 7 + Double(y) * 13 + (t * 4).rounded(.down)) > 0.45 { g.put(x, Double(y), "=", "#ffb07a", 0.85 * (1 - f)) }
                    x += 1
                }
            }
            let bx = Ascii.mod(t * 1.3, cd + 14) - 10
            let by = hzd - 3 + sin(t * 1.8) * 0.5
            g.sprite(bx, by, ["   |\\", "   | \\", "   |__\\"], "#fff7ed", 0.95)
            g.sprite(bx, by + 3, ["\\_______/"], "#3a2a2a", 0.95)
        }
    }

    static let lanterns = AsciiScene(
        id: "ascii-lanterns", name: "Lanterns",
        note: "Lanterns rise from a dark lake and glow in its water",
        bg: ["#0f0a1e", "#2d1430", "#3a1a2c"], tile: true
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let S = Ascii.stars(R, c, rd * 0.6, (cd * rd / 70) * D)
        let lake = Int((rd * 0.78).rounded())
        let span = Double(lake + 6)
        let L = (0..<Ascii.count(cd * rd / 80, D, 6)).map { _ in (x: R() * cd, y: R() * span, v: 0.4 + R(), p: R() * tau, z: R()) }
        return { g, t, _ in
            Ascii.drawStars(g, S, t, "#f5d0fe")
            for l in L {
                let y = Ascii.wrap(l.y - t * l.v * (0.5 + l.z), span) - 4
                let x = l.x + sin(t * 0.5 + l.p) * 2
                let fl = 0.85 + 0.15 * sin(t * 7 + l.p * 3)
                if l.z > 0.65 {
                    g.text(x, y, ".-.", "#ffd08a", 0.6)
                    for i in 0..<3 {
                        g.put(x + Double(i), y + 1, " ", i == 1 ? "#ffcf6b" : "#ffb347", fl)
                        g.put(x + Double(i), y + 2, " ", i == 1 ? "#ffb347" : "#f59e0b", fl)
                    }
                    for o in [(-1.0, 1.0), (3, 1), (-1, 2), (3, 2), (1, 3), (1, -1)] { g.put(x + o.0, y + o.1, "·", "#ff9f43", 0.4) }
                } else if l.z > 0.3 {
                    for i in 0..<2 {
                        g.put(x + Double(i), y, " ", "#ffb347", fl)
                        g.put(x + Double(i), y + 1, " ", "#f59e0b", fl)
                    }
                } else {
                    g.put(x, y, "o", "#ffcf80", 0.6 * fl)
                }
            }
            for y in lake..<r {
                for x in 0..<c {
                    let my = 2 * lake - 1 - y
                    if my < 0 { continue }
                    let xd = Double(x), yd = Double(y)
                    if let cell = g.cell(my * c + x), cell.a > 0.5, Ascii.hash(xd * 13 + yd * 7 + (t * 6).rounded(.down)) > 0.35 {
                        g.put(xd + sin(yd + t * 2).rounded(), yd, "~", cell.col, 0.45)
                    } else if (x + y * 3 + Int((t * 3).rounded(.down))) % 9 == 0 {
                        g.put(xd, yd, "-", "#6b3a5a", 0.3)
                    }
                }
            }
        }
    }

    static let snowfall = AsciiScene(
        id: "ascii-snowfall", name: "Snowfall",
        note: "Layers of snow drift past pines and far mountains",
        bg: ["#7f97b3", "#c9d6e4", "#eef3f8"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        func flakes(_ n: Double, _ z: Double) -> [(x: Double, y: Double, v: Double, p: Double)] {
            (0..<Ascii.count(n, D, 6)).map { _ in (R() * cd, R() * rd, (0.6 + R()) * (1 + z * 2), R() * tau) }
        }
        let far = flakes(cd * rd / 40, 0)
        let near = flakes(cd * rd / 80, 1)
        let p1 = R() * 9
        let mtn = (0..<c).map { x in Int((rd * 0.3 + sin(Double(x) * 0.06 + p1) * rd * 0.08 + Ascii.fbm(Double(x) * 0.2, 0, 3) * rd * 0.05).rounded()) }
        func ground(_ x: Double) -> Int { Int((rd * 0.12 + sin(x * 0.09) * rd * 0.03).rounded()) }
        let pines = (0..<max(3, Int((cd / 11).rounded()))).map { _ in (x: (R() * cd).rounded(.down), h: Int(3 + R() * max(2, rd * 0.16))) }
        return { g, t, _ in
            for x in 0..<c {
                for y in max(0, r - mtn[x])..<r {
                    g.put(Double(x), Double(y), y == r - mtn[x] ? "^" : ":", y < r - mtn[x] + 2 ? "#ffffff" : "#9fb2c8", 0.7)
                }
            }
            for f in far { g.put(f.x + sin(t * 0.6 + f.p), Ascii.mod(f.y + t * f.v, rd), ".", "#ffffff", 0.6) }
            for p in pines {
                let base = Double(r - ground(p.x))
                for k in 0..<p.h {
                    let w = Int((Double(p.h - k) * 0.7).rounded())
                    for i in -w...w { g.put(p.x + Double(i), base - 1 - Double(k), i < 0 ? "/" : i > 0 ? "\\" : "|", k % 2 == 1 ? "#2c4a3e" : "#35574a", 1) }
                }
                g.put(p.x, base - 1 - Double(p.h), "^", "#2c4a3e", 1)
            }
            for x in 0..<c {
                let h = ground(Double(x))
                for y in max(0, r - h)..<r { g.put(Double(x), Double(y), y == r - h ? "_" : "·", "#ffffff", 0.95) }
            }
            for f in near { g.put(f.x + sin(t * 0.9 + f.p) * 2, Ascii.mod(f.y + t * f.v, rd), "*", "#ffffff", 0.95) }
        }
    }

    static let orbit = AsciiScene(
        id: "ascii-orbit", name: "Orbit",
        note: "Planets circle a glowing sun with trails behind them",
        bg: ["#06060e", "#161628"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let S = Ascii.stars(R, c, rd, (cd * rd / 50) * D)
        let cx = cd / 2, cy = rd / 2
        let ky = (rd / 2 - 1) / 18
        let kx = min(ky * 2.1, (cd / 2 - 1) / 18)
        let planets: [(d: Double, s: Double, ch: Character, col: String)] = [(7, 0.7, "o", "#60a5fa"), (12, 0.42, "O", "#f472b6"), (17, 0.25, "0", "#a3e635")]
        func pos(_ d: Double, _ a: Double) -> (Double, Double) { (cx + cos(a) * d * kx, cy + sin(a) * d * ky) }
        let spokes = "-\\|/"
        return { g, t, _ in
            Ascii.drawStars(g, S, t, "#c7d2fe")
            for p in planets {
                var a = 0.0
                while a < tau {
                    let (x, y) = pos(p.d, a)
                    g.put(x, y, ".", "#8b8bb5", 0.25)
                    a += 0.04
                }
            }
            let sr = max(1.2, ky * 3.5)
            var y = -(sr * 1.5).rounded(.up)
            while y <= sr * 1.5 {
                var x = -(sr * 3).rounded(.up)
                while x <= sr * 3 {
                    let d = hypot(x / 2, y)
                    if d < sr { g.put(cx + x, cy + y, "@", "#ffd166", 1) }
                    else if d < sr + 1 {
                        let k = (((Int(((atan2(y, x / 2) / tau) * 8 + t * 3).rounded(.down))) % 4) + 4) % 4
                        g.put(cx + x, cy + y, ch(spokes, k), "#ffb347", 0.6)
                    }
                    x += 1
                }
                y += 1
            }
            for (i, p) in planets.enumerated() {
                for j in stride(from: 8, through: 0, by: -1) {
                    let (x, y) = pos(p.d, t * p.s + Double(i) * 2 - Double(j) * 0.06)
                    g.put(x, y, j > 0 ? "." : p.ch, p.col, j > 0 ? 0.5 * (1 - Double(j) / 9) : 1)
                }
            }
            let (mx, my) = pos(planets[1].d, t * planets[1].s + 2)
            g.put(mx + cos(t * 2.5) * 3, my + sin(t * 2.5) * 1.5, "·", "#e5e7eb", 0.9)
        }
    }

    static let garden = AsciiScene(
        id: "ascii-garden", name: "Garden",
        note: "Grass sways, flowers open and two butterflies wander",
        bg: ["#f7fcf1", "#c9e6bd"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let flowers: [Character] = ["*", "@", "o"]
        let fcols = ["#e0559a", "#f2a93b", "#8b5cf6"]
        let blades = (0..<c).map { _ -> (h: Int, p: Double, f: Character?, fc: String) in
            let h = Int(2 + R() * max(3, rd * 0.3 * D))
            let p = R() * tau
            let f: Character? = R() < 0.18 * D ? flowers[Int(R() * 3)] : nil
            return (h, p, f, fcols[Int(R() * 3)])
        }
        let flies = [0, 1].map { i in (a: R() * tau, cx: cd * (0.3 + 0.4 * Double(i)), col: i == 1 ? "#f2a93b" : "#e0559a") }
        return { g, t, _ in
            for (x, b) in blades.enumerated() {
                let xd = Double(x)
                let sw0 = sin(t * 1.3 + xd * 0.25 + b.p * 0.3)
                for k in 0..<b.h {
                    let sw = sw0 * (Double(k) / Double(b.h))
                    g.put(xd + (sw * 1.2).rounded(), rd - 1 - Double(k), sw > 0.3 ? "/" : sw < -0.3 ? "\\" : "|", "#3f8f4a", 0.55 + (0.45 * Double(k)) / Double(b.h))
                }
                if let f = b.f { g.put(xd + (sw0 * 1.2).rounded(), rd - 1 - Double(b.h), 0.5 + 0.5 * sin(t * 0.4 + b.p) > 0.3 ? f : ".", b.fc, 1) }
            }
            for (i, b) in flies.enumerated() {
                g.text(b.cx + sin(t * 0.5 + b.a) * cd * 0.2, rd * 0.35 + sin(t * 0.9 + b.a * 2) * rd * 0.15, sin(t * 12 + Double(i)) > 0 ? "}{" : "><", b.col, 1)
            }
        }
    }

    static let ripple = AsciiScene(
        id: "ascii-ripple", name: "Ripple Grid",
        note: "A quiet dot grid; drops send rings across it",
        bg: ["#131417", "#202227"]
    ) { c, r, _, D in
        let cd = Double(c), rd = Double(r)
        let RAMP = " .·:-=+*#"
        let per = 1.3 / D
        return { g, t, _ in
            let now = Int((t / per).rounded(.down))
            let src = ((now - 4)...now).map { k in (x: Ascii.hash(Double(k * 3 + 1)) * cd, y: Ascii.hash(Double(k * 3 + 2)) * rd, t0: Double(k) * per) }
            for y in 0..<r {
                var x = 0
                while x < c {
                    var h = 0.0
                    for s in src {
                        let age = t - s.t0
                        if age < 0 { continue }
                        let d = hypot((Double(x) - s.x) / 2, Double(y) - s.y)
                        h += exp(-pow(d - age * 7, 2) / 2.5) * exp(-age * 0.5)
                    }
                    if h > 0.08 { g.put(Double(x), Double(y), ch(RAMP, min(8, 1 + Int(h * 8))), "#F2A93B", 0.35 + h) }
                    else { g.put(Double(x), Double(y), ".", "#9ca3af", 0.18) }
                    x += 2
                }
            }
        }
    }

    static let meteors = AsciiScene(
        id: "ascii-meteors", name: "Meteor Night",
        note: "Shooting stars cross the sky above the hills",
        bg: ["#0b0a1d", "#33224b"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let S = Ascii.stars(R, c, rd, (cd * rd / 30) * D, top: 0.75)
        let p1 = R() * 9, p2 = R() * 9
        let h1 = (0..<c).map { x in Int((rd * 0.18 + sin(Double(x) * 0.05 + p1) * rd * 0.08 + sin(Double(x) * 0.13 + p2) * rd * 0.03).rounded()) }
        let h2 = (0..<c).map { x in Int((rd * 0.1 + sin(Double(x) * 0.08 + p2) * rd * 0.06).rounded()) }
        return { g, t, _ in
            Ascii.drawStars(g, S, t, "#fef3c7")
            for m in 0..<3 {
                let md = Double(m)
                let per = 3.5 + md * 1.3
                let q = t + md * 1.1
                let cyc = (q / per).rounded(.down)
                let ph = Ascii.mod(q, per)
                if ph >= 1.4 { continue }
                let x = Ascii.hash(cyc * 7 + md) * cd * 0.7 + ph * cd * 0.35
                let y = Ascii.hash(cyc * 11 + md) * rd * 0.3 + ph * rd * 0.22
                for j in 0..<9 { g.put(x - Double(j) * 1.6, y - Double(j) * 0.6, j > 0 ? "\\" : "*", j > 0 ? "#c4b5fd" : "#fff7d6", j > 0 ? (1 - Double(j) / 9) * 0.8 : 1) }
            }
            for x in 0..<c {
                for y in max(0, r - h1[x])..<r { g.put(Double(x), Double(y), ":", "#4b3a6e", 0.9) }
                for y in max(0, r - h2[x])..<r { g.put(Double(x), Double(y), "#", "#1d1530", 1) }
            }
        }
    }

    static let kelp = AsciiScene(
        id: "ascii-kelp", name: "Kelp Forest",
        note: "Kelp sways while small fish swim through",
        bg: ["#0f5566", "#04202a"]
    ) { c, r, R, D in
        let cd = Double(c), rd = Double(r)
        let K = (0..<Ascii.count(cd / 6, D, 3)).map { _ in (x: R() * cd, h: rd * (0.35 + R() * 0.5), p: R() * tau) }
        let fishCols = ["#fbbf24", "#fb7185", "#e0f2fe"]
        let F = (0..<Ascii.count(rd / 5, D, 3)).map { _ in (y: 2 + R() * (rd - 6), v: 2 + R() * 4, o: R() * 100, d: R() < 0.5 ? 1.0 : -1.0, col: fishCols[Int(R() * 3)]) }
        let bubbles = (0..<Ascii.count(cd / 8, D, 4)).map { _ in (x: R() * cd, y: R() * rd, v: 1.5 + R() * 2) }
        return { g, t, _ in
            for k in K {
                var i = 0.0
                while i < k.h {
                    g.put(k.x + sin(i * 0.18 + t * 1.1 + k.p) * (i / k.h) * 3, rd - 1 - i, Int(i) % 2 == 1 ? "(" : ")", "#3fae6a", 0.85)
                    i += 1
                }
            }
            for f in F {
                let p = Ascii.mod(f.o + t * f.v, cd + 8)
                g.text(f.d > 0 ? p - 4 : cd - p, f.y + sin(t + f.o) * 0.5, f.d > 0 ? "><>" : "<><", f.col, 1)
            }
            for b in bubbles { g.put(b.x + sin(t * 2 + b.x), Ascii.wrap(b.y - t * b.v, rd), "o", "#bdf2ff", 0.5) }
        }
    }

    /// In picker order: Allternit, moods, scenes.
    static let all: [AsciiScene] = [
        aBlock, clay, inkBloom, spectrum, eventHorizon, lattice, ember, skyDrift, glyphStars, rainGlass,
        codeRain, northernLights, tide, lanterns, snowfall, orbit, garden, ripple, meteors, kelp,
    ]

    private static let byId: [String: AsciiScene] = Dictionary(uniqueKeysWithValues: all.map { ($0.id, $0) })

    static func scene(_ id: String) -> AsciiScene? { byId[id] }
}
