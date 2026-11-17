// Window geometry: capture pixels ↔ global screen points, and the rule that
// input never leaves the iPhone Mirroring window.

import CoreGraphics
import Foundation

public struct WindowGeometry: Equatable, Sendable {
    /// Window frame in global display points (top-left origin, as CGWindowList and CGEvent use).
    public let bounds: CGRect
    /// Backing pixels per point of the display the window is on.
    public let scale: Double

    public init(bounds: CGRect, scale: Double) {
        self.bounds = bounds
        self.scale = scale > 0 ? scale : 1
    }

    /// Size of a `capture` image of this window.
    public var pixelWidth: Int { Int((Double(bounds.width) * scale).rounded()) }
    public var pixelHeight: Int { Int((Double(bounds.height) * scale).rounded()) }

    /// A capture-pixel point → a global screen point. Refuses anything outside the window.
    /// A coordinate for an error message. `Int(_:)` traps on NaN or infinity, which are
    /// exactly the values the guard below rejects (build train 20261117-0034).
    static func coord(_ v: Double) -> String { v.isFinite ? String(Int(v)) : String(describing: v) }

    public func screenPoint(x: Double, y: Double) throws -> CGPoint {
        guard x.isFinite, y.isFinite, x >= 0, y >= 0, x < Double(pixelWidth), y < Double(pixelHeight) else {
            throw MirrorError(
                "outside_window",
                "(\(Self.coord(x)), \(Self.coord(y))) is outside the iPhone Mirroring window (\(pixelWidth)×\(pixelHeight) pixels). Take a new screenshot and use its coordinates."
            )
        }
        let p = CGPoint(x: Double(bounds.minX) + x / scale, y: Double(bounds.minY) + y / scale)
        // Belt and braces: the converted point must still be inside the frame.
        guard bounds.contains(p) else {
            throw MirrorError("outside_window", "The point falls outside the iPhone Mirroring window.")
        }
        return p
    }

    /// Start and end pixels of a swipe from the centre (the finger moves in `direction`).
    public func swipe(_ direction: Direction) -> (Double, Double, Double, Double) {
        let w = Double(pixelWidth), h = Double(pixelHeight)
        let cx = w / 2, cy = h / 2
        switch direction {
        case .up: return (cx, cy + h * 0.25, cx, cy - h * 0.25)
        case .down: return (cx, cy - h * 0.25, cx, cy + h * 0.25)
        case .left: return (cx + w * 0.3, cy, cx - w * 0.3, cy)
        case .right: return (cx - w * 0.3, cy, cx + w * 0.3, cy)
        }
    }

    /// iOS back: a swipe in from the left edge at mid height.
    public var backSwipe: (Double, Double, Double, Double) {
        let w = Double(pixelWidth), h = Double(pixelHeight)
        return (max(1, w * 0.01), h * 0.5, w * 0.6, h * 0.5)
    }

    /// A Vision bounding box (normalized, bottom-left origin) → [x1, y1, x2, y2] in capture pixels.
    public func pixelBox(normalized r: CGRect) -> [Int] {
        MirrorCore.pixelBox(normalized: r, width: pixelWidth, height: pixelHeight)
    }
}

/// A Vision box (normalized, bottom-left origin) as [x1,y1,x2,y2] pixels of a `width`×`height` image, top-left origin.
public func pixelBox(normalized r: CGRect, width: Int, height: Int) -> [Int] {
    let w = Double(width), h = Double(height)
    let x1 = Double(r.minX) * w, x2 = Double(r.maxX) * w
    let y1 = (1 - Double(r.maxY)) * h, y2 = (1 - Double(r.minY)) * h
    func clampX(_ v: Double) -> Int { Int(min(max(v, 0), w).rounded()) }
    func clampY(_ v: Double) -> Int { Int(min(max(v, 0), h).rounded()) }
    return [clampX(x1), clampY(y1), clampX(x2), clampY(y2)]
}

/// Another window that sits above the Mirroring window at a point (from CGWindowList, front to back).
public struct OverlappingWindow: Equatable, Sendable {
    public let owner: String
    public let layer: Int
    public let alpha: Double
    public let bounds: CGRect
    public init(owner: String, layer: Int, alpha: Double, bounds: CGRect) {
        self.owner = owner
        self.layer = layer
        self.alpha = alpha
        self.bounds = bounds
    }
}

/// The first visible window above ours that covers `point`, if any: a click there would land in another app.
public func coveringWindow(at point: CGPoint, above: [OverlappingWindow]) -> OverlappingWindow? {
    above.first { $0.layer >= 0 && $0.alpha > 0.01 && $0.bounds.contains(point) }
}
