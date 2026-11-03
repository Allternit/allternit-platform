// Allternit addition (not upstream): the five ink types of the web
// voice-ring library — orbit, sash, signal, swell, pixel — ported line for
// line from packages/voice-ring/src/engine/*.ts at the commit noted in
// Package.swift, with the tunings from src/presets.ts. They draw on the same
// VoiceCore engine, hashes, noise and projection as the Ring, so iPhone
// shows the same looks as the web app.
import SwiftUI
import VoiceCore

/// The ink types (the Ring is `VoiceRing`).
public enum OrbType: String, CaseIterable, Sendable {
    case orbit, sash, signal, swell, pixel
}

/// A voice-reactive orb of one ink type.
public struct OrbView: View {
    private let type: OrbType
    private let state: VoiceState
    private let source: VoiceSource
    private let style: VoiceStyle
    private let size: CGFloat

    public init(type: OrbType, state: VoiceState = .listening, source: VoiceSource = .silence, style: VoiceStyle = VoiceStyle(), size: CGFloat = 200) {
        self.type = type
        self.state = state
        self.source = source
        self.style = style
        self.size = size
    }

    public var body: some View {
        let style = self.style
        let type = self.type
        VoiceCanvas(
            resolve: { OrbView.preset(type, size: $0, dots: style.dots, dotSize: style.dotSize ?? 1) },
            state: state, source: source, style: style, mood: nil, direction: nil,
            size: size, colourful: true
        )
    }

    /// src/presets.ts, per type.
    public static func preset(_ type: OrbType, size: Double, dots: Double = 1, dotSize: Double = 1) -> ResolvedPreset {
        switch type {
        case .orbit:
            return .resolve(frame: frameOrbit, speed: 1.05, driveGain: 0.12, opts: ["orbitN": 12, "particles": 6],
                            counts: [:], size: size, dots: dots, dotSize: dotSize)
        case .sash:
            return .resolve(frame: frameSash, speed: 1.5, driveGain: 0.8, gain: 1.1, opts: ["lanes": 10, "segs": 56, "ghostN": 60],
                            counts: ["segs": 0.45, "ghostN": 0.8], size: size, dots: dots, dotSize: dotSize)
        case .signal:
            return .resolve(frame: frameSignal, speed: 1.6, driveGain: 2, opts: ["circles": 8, "segs": 96],
                            counts: ["circles": 0.5, "segs": 0.5], size: size, dots: dots, dotSize: dotSize)
        case .swell:
            return .resolve(frame: frameSwell, speed: 1.4, driveGain: 2.4, opts: ["dotN": 900],
                            counts: ["dotN": 1], size: size, dots: dots, dotSize: dotSize)
        case .pixel:
            return .resolve(frame: framePixel, speed: 1.2, driveGain: 1.6, attack: 0.16, opts: ["cells": 17],
                            counts: ["cells": 0.5], size: size, dots: dots, dotSize: dotSize * 0.55)
        }
    }
}

// MARK: - orbit.ts

let frameOrbit: TypeFrame = { size, t, v, o in
    let cx = size / 2, cy = size / 2
    let voice = 0.5 * v.level + 0.5 * v.soft
    let zoom = (1 + o.v("swell", 0.49) * voice) * (1 - 0.14 * v.processing)
    let R = (size / 2) * o.v("radius", 0.56)
    let persp = o.v("persp", 0.25)
    let pt = Projector(yaw: t * 0.12 + v.drive * 0.03, tilt: 0.3, cx: 0, cy: 0, scale: 1)
    let rs = radiusScale(size, o.v("rsPow", 0.9)) * o.v("rMul", 1)
    var dots: [Dot] = []
    let orbitN = Int(o.v("orbitN", 12)), particles = Int(o.v("particles", 6))
    for orb in 0..<orbitN {
        let od = Double(orb)
        let h1 = hashD(od, 1.7), h2 = hashD(od, 5.2), h3 = hashD(od, 8.9)
        let ro = R * (0.45 + 0.52 * h1)
        let th = h1 * TAU
        let phi = acos(2 * h2 - 1)
        let nx = sin(phi) * cos(th), ny = cos(phi), nz = sin(phi) * sin(th)
        var ux = -ny, uy = nx
        let ul = max(1e-6, (ux * ux + uy * uy).squareRoot())
        ux /= ul; uy /= ul
        let vx = -nz * uy, vy = nz * ux, vz = nx * uy - ny * ux
        let speed = (0.25 + 0.55 * h3) * (h3 > 0.5 ? 1 : -1)
        for m in 0..<particles {
            let a = v.drive * speed + Double(m) / Double(particles) * TAU + h2 * 6
            let ca = cos(a), sa = sin(a)
            let (x, y, z) = pt((ux * ca + vx * sa) * ro, (uy * ca + vy * sa) * ro, vz * sa * ro)
            let depth = clamp01((z / R + 1) / 2)
            let k = zoom / (1 - (persp * z) / R)
            dots.append(Dot(x: cx + x * k, y: cy + y * k, z: z,
                            r: (o.v("partR", 0.47) + o.v("partRDepth", 0.65) * depth) * rs * k,
                            white: 0.32 - 0.24 * depth))
        }
    }
    return finalizeFrame(dots, [], rMin: o.v("rMin", 0.3))
}

// MARK: - sash.ts

let frameSash: TypeFrame = { size, t, v, o in
    let cx = size / 2, cy = size / 2
    let L = v.soft
    let lo = v.softBands[0], mid = v.softBands[1]
    let R = (size / 2) * o.v("radius", 0.66) * (1 + o.v("zoom", 0.21) * v.level)
    let T0 = v.drive
    let deform = o.v("deform", 1), deepen = o.v("wave", 1)
    let aLobe = 0.015 + (0.1 * lo + 0.05 * v.level) * deform
    let aFine = 0.04 * mid * deform
    let skin = skinField(aLobe, aFine, T0)
    let ta = 0.55 + 0.05 * sin(t * 0.37)
    let vy = cos(ta), vz = sin(ta)
    var dc = 0.0
    for i in 0..<48 {
        let a = Double(i) / 48 * TAU
        dc += skin(cos(a), sin(a) * vy, sin(a) * vz)
    }
    dc /= 48
    let bulge: (Double, Double, Double) -> Double = { x, y, z in 1 + skin(x, y, z) - dc }
    let pt = Projector(yaw: 0, tilt: 0.3, cx: cx, cy: cy, scale: 1)
    let rs = radiusScale(size, o.v("rsPow", 0.6)) * o.v("rMul", 1)
    let T = v.drive
    var dots: [Dot] = []
    let ghostN = Int(o.v("ghostN", 60))
    for i in 0..<ghostN {
        let d = fibDir(i, ghostN)
        let rr = R * bulge(d.x, d.y, d.z)
        let (px, py, z) = pt(d.x * rr, d.y * rr, d.z * rr)
        let depth = (z / R + 1) / 2
        dots.append(Dot(x: px, y: py, z: z, r: 0.65 * rs, white: 0.78, a: 0.1 + 0.22 * depth))
    }
    let ny = -vz, nz = vy
    let lanes = max(1, Int(o.v("lanes", 10).rounded()))
    let segs = max(8, Int(o.v("segs", 56).rounded()))
    let P = v.processing
    let laneGap = o.v("laneGap", 0.062) * (1 + 0.25 * L) * (1 - 0.4 * P)
    let pulseAt = T * 1.6
    let aLong = 0.15 * (0.55 + 0.9 * lo * deepen) * (1 - 0.6 * P)
    let aShort = 0.06 * (0.5 + 1.2 * mid * deepen)
    let aSwell = 0.05 + 0.05 * L
    let half = Double(lanes - 1) / 2
    for w in 0..<lanes {
        let wd = Double(w)
        let laneOff = (wd - half) * laneGap
        let edge = abs(wd - half) / max(1, half)
        for k in 0..<segs {
            let a = Double(k) / Double(segs) * TAU
            let wob = aLong * sin(a * 3 - T * 0.95 + wd * 0.18)
                + aShort * sin(a * 5 + T * 0.6)
                + aSwell * sin(a * 2 + t * 0.31 + sin(t * 0.17) * 1.5)
            let off = laneOff + wob
            let x = cos(a)
            let y = sin(a) * vy + ny * off
            let z = sin(a) * vz + nz * off
            let l = (x * x + y * y + z * z).squareRoot()
            let rr = (R * bulge(x / l, y / l, z / l)) / l
            let (px, py, zr) = pt(x * rr, y * rr, z * rr)
            let depth = (zr / R + 1) / 2
            let crest = max(0, wob) * L
            let da = atan2(sin(a - pulseAt), cos(a - pulseAt))
            let pulse = exp(-(da * da) / 0.18) * P
            dots.append(Dot(x: px, y: py, z: zr,
                            r: (o.v("rBase", 0.49) + o.v("rDepth", 0.78) * depth) * (1 - 0.25 * edge) * (1 + 1.2 * crest + 0.5 * pulse) * rs,
                            white: 0.52 - 0.44 * depth + 0.18 * edge - 0.3 * pulse,
                            a: 0.4 + 0.6 * depth))
        }
    }
    return finalizeFrame(dots, [], rMin: o.v("rMin", 0.3))
}

// MARK: - signal.ts

let frameSignal: TypeFrame = { size, t, v, o in
    let half = size / 2
    let L = v.level
    let lo = v.softBands[0], mid = v.softBands[1]
    let hi = v.bands[2]
    let T = v.drive
    let R = half * o.v("radius", 0.6) * (1 + o.v("zoom", 0.06) * v.soft) * (1 - 0.08 * v.processing)
    let pt = Projector(yaw: o.v("yaw", 0.42) + 0.08 * sin(t * 0.23), tilt: o.v("tilt", 0.32), cx: half, cy: half, scale: 1)
    let lw = max(0.4, o.v("lineW", 0.6) * radiusScale(size, 0.5))
    let depthA: (Double) -> Double = { z in 0.06 + 0.94 * pow(clamp01((z / R + 1) / 2), 2) }
    let deform = o.v("deform", 1)
    let field = skinField(0.02 + (0.14 * lo + 0.06 * L) * deform, 0.01 + (0.07 * mid + 0.03 * hi) * deform, T)
    var strokes: [Stroke] = []
    let segs = max(24, Int(o.v("segs", 96).rounded()))
    let limbs = max(0, Int(o.v("limbs", 2).rounded()))
    for k in 0..<limbs {
        let kd = Double(k)
        let lean = 0.1 + 0.05 * sin(t * 0.31 + kd * 2.1)
        let dir = t * 0.17 * (k % 2 == 1 ? 1 : -1) + kd * 2.4
        let nx = sin(lean) * cos(dir), ny = sin(lean) * sin(dir), nz = cos(lean)
        let ul = hypot(nx, nz)
        let ux = nz / ul, uz = -nx / ul
        let vx = ny * uz, vy = nz * ux - nx * uz, vz = -ny * ux
        var pts: [Double] = [], al: [Double] = []
        for i in 0..<segs {
            let th = Double(i) / Double(segs) * TAU
            let x = ux * cos(th) + vx * sin(th), y = vy * sin(th), z = uz * cos(th) + vz * sin(th)
            let r = R * (1 + 0.006 * kd + field(x, y, z))
            pts.append(half + x * r); pts.append(half - y * r)
            al.append(0.55 + 0.35 * clamp01((z / sin(lean) + 1) / 2))
        }
        strokes.append(Stroke(pts: pts, a: al, white: 0, w: lw, closed: true))
    }
    let circles = max(2, Int(o.v("circles", 8).rounded()))
    for k in 0..<circles {
        let kd = Double(k)
        let n0 = fibDir(k, circles)
        let a = T * 0.16 * (0.5 + hashD(kd, 2.1)) * (k % 2 == 1 ? 1 : -1) + hashD(kd, 6.6) * TAU
        let nx = n0.x
        let ny = n0.y * cos(a) - n0.z * sin(a)
        let nz = n0.y * sin(a) + n0.z * cos(a)
        let ul = max(1e-6, hypot(nz, nx))
        let ux = nz / ul, uz = -nx / ul
        let vx = ny * uz, vy = nz * ux - nx * uz, vz = -ny * ux
        var pts: [Double] = [], al: [Double] = []
        for i in 0..<segs {
            let th = Double(i) / Double(segs) * TAU
            let x = ux * cos(th) + vx * sin(th), y = vy * sin(th), z = uz * cos(th) + vz * sin(th)
            let r = R * (1 + field(x, y, z))
            let (px, py, zr) = pt(x * r, y * r, z * r)
            pts.append(px); pts.append(py)
            al.append(depthA(zr))
        }
        strokes.append(Stroke(pts: pts, a: al, white: 0, w: lw, closed: true))
    }
    return finalizeFrame([], strokes, rMin: o.v("rMin", 0.3))
}

// MARK: - swell.ts

let frameSwell: TypeFrame = { size, t, v, o in
    let half = size / 2
    let L = v.level
    let lo = v.bands[0], mid = v.bands[1], hi = v.bands[2]
    let T = v.drive
    let P = v.processing
    let R = half * o.v("radius", 0.6) * (1 + o.v("zoom", 0.1) * L) * (1 - 0.08 * P)
    let scanY = 0.95 * sin(t * 1.3)
    let pt = Projector(yaw: t * 0.14, tilt: 0.32, cx: half, cy: half, scale: 1)
    let rs = radiusScale(size, o.v("rsPow", 0.6)) * o.v("rMul", 1)
    let n = max(40, Int(o.v("dotN", 900).rounded()))
    let freq = o.v("cloudFreq", 2.3)
    let deform = o.v("deform", 1)
    let aLobe = 0.018 + (0.12 * lo + 0.04 * L) * deform
    let aFine = (0.045 * mid + 0.03 * hi) * deform
    var dirs: [SIMD3<Double>] = []
    var disps: [Double] = []
    dirs.reserveCapacity(n); disps.reserveCapacity(n)
    var mean = 0.0
    for i in 0..<n {
        let d = fibDir(i, n)
        let lobes = 0.55 * sin(2.1 * d.x + 1.3 * d.y + 0.9 * T) + 0.45 * sin(1.7 * d.z - 1.9 * d.y - 0.7 * T + 1.3)
        let fine = sin(5.3 * d.x - 4.1 * d.z + 2.3 * T) * sin(4.7 * d.y + 3.1 * d.x - 1.7 * T)
        let disp = aLobe * lobes + aFine * fine
        dirs.append(d); disps.append(disp)
        mean += disp / Double(n)
    }
    var dots: [Dot] = []
    dots.reserveCapacity(n)
    for i in 0..<n {
        let d = dirs[i]
        let disp = disps[i] - mean
        let rr = R * (1 + disp)
        let (px, py, z) = pt(d.x * rr, d.y * rr, d.z * rr)
        let depth = clamp01((z / R + 1) / 2)
        let c = 0.65 * vnoise3(d.x * freq + T * 0.21, d.y * freq - T * 0.15, d.z * freq + T * 0.1)
            + 0.35 * vnoise3(d.x * freq * 2.1 - T * 0.29, d.y * freq * 2.1 + 7.3, d.z * freq * 2.1 + T * 0.24)
        let cloud = smoothstep(0.34, 0.68, c)
        let scan = exp(-pow(d.y - scanY, 2) / 0.012) * P
        let bulge = max(0, disp) / 0.16
        dots.append(Dot(x: px, y: py, z: z,
                        r: (o.v("rBase", 0.27) + o.v("rDepth", 0.84) * depth) * (0.22 + 0.95 * cloud + 0.6 * scan) * (1 + 0.5 * bulge) * rs,
                        white: 0.62 - 0.52 * depth - 0.16 * cloud - 0.3 * scan,
                        a: 0.3 + 0.7 * depth))
    }
    return finalizeFrame(dots, [], rMin: o.v("rMin", 0.25))
}

// MARK: - pixel.ts

private let LX = -0.45, LY = 0.55, LZ = 0.7
private let LN = (LX * LX + LY * LY + LZ * LZ).squareRoot()

let framePixel: TypeFrame = { size, t, v, o in
    let half = size / 2
    let L = v.level
    let lo = v.bands[0], mid = v.bands[1], hi = v.bands[2]
    let T = v.drive
    let P = v.processing
    let base = half * o.v("radius", 0.72)
    let R = base * (1 + o.v("zoom", 0.3) * L) * (1 - 0.06 * P)
    let rMul = o.v("rMul", 1)
    let n = max(4, Int(o.v("cells", 17).rounded()))
    let cell = (2 * base) / Double(n)
    let most = cell / 2 - max(0.5, (cell * o.v("gap", 0.24)) / 2)
    let reach = Int(ceil((R * 1.25) / cell))
    let yaw = o.v("turn", 0.12) * t + 0.1 * T + 0.5 * v.spin
    let sy = sin(yaw), cyw = cos(yaw)
    let st = sin(0.35), ct = cos(0.35)
    let shade = o.v("shade", 0.7)
    let tex = o.v("texture", 0.35)
    let fade = o.v("fade", 1.3)
    let strength = o.v("strength", 1.38)
    let deform = o.v("deform", 1.7)
    let aLobe = 0.012 + (0.09 * lo + 0.03 * L) * deform
    let aFine = (0.035 * mid + 0.025 * hi) * deform
    let rim: (Double) -> Double = { a in
        aLobe * (0.55 * sin(2 * a + 0.9 * T) + 0.45 * sin(3 * a - 0.7 * T + 1.3)) + aFine * sin(7 * a + 2.3 * T) * sin(5 * a - 1.7 * T)
    }
    let scanY = 0.95 * sin(t * 1.3)
    let spectrum = v.spectrum
    let slots = spectrum.count
    var dots: [Dot] = []
    for j in -reach...reach {
        for i in -reach...reach {
            let x = half + Double(i) * cell
            let y = half + Double(j) * cell
            let nx = (x - half) / R
            let ny = -(y - half) / R
            let bulge = rim(atan2(ny, nx))
            let rho = hypot(nx, ny) / (1 + bulge)
            if rho >= 1 { continue }
            let k = 1 / (1 + bulge)
            let vx = nx * k, vy = ny * k
            let vz = max(0, 1 - vx * vx - vy * vy).squareRoot()
            let lambert = clamp01((vx * LX + vy * LY + vz * LZ) / LN)
            let lit = 1 - shade + shade * lambert
            let gy = vy * ct + vz * st
            let z1 = -vy * st + vz * ct
            let gx = vx * cyw - z1 * sy
            let gz = vx * sy + z1 * cyw
            let c = 0.65 * vnoise3(gx * 2.2 + T * 0.15, gy * 2.2, gz * 2.2 - T * 0.1)
                + 0.35 * vnoise3(gx * 4.6 - T * 0.2, gy * 4.6 + 7.3, gz * 4.6)
            let surface = 1 - tex + tex * smoothstep(0.32, 0.68, c)
            let ring = slots > 0 ? spectrum[min(slots - 1, Int(floor(rho * Double(slots))))] : L
            let voice = 0.35 * L + 0.85 * ring
            let scan = exp(-pow(vy - scanY, 2) / 0.01) * P
            let edge = 1 - 0.4 * fade * (1 - vz)
            let fill = (0.35 + 0.75 * lit * surface + 0.3 * voice + 0.2 * scan) * edge * rMul
            let ink = 0.22 + 0.62 * lit * surface + 0.22 * voice + 0.3 * scan
            dots.append(Dot(x: x, y: y, z: vz * R, r: most * min(1, fill),
                            white: clamp01(1 - ink * strength),
                            a: clamp01((1 - 0.75 * fade * (1 - vz.squareRoot())) * strength)))
        }
    }
    var frame = finalizeFrame(dots, [], rMin: o.v("rMin", 0.4))
    frame.square = true
    return frame
}
