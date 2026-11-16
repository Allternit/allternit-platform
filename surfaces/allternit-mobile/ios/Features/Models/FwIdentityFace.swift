import SwiftUI

// The Freeweights identity face (the site's "element tile", fw-cards.js
// `faceIdentity` + fw.css `.fwf-id`), ported to SwiftUI so a model looks the
// same on freeweights.allternit.com, in the web app and on the iPhone:
// params line + publisher logo, the family's two dot letters, the size, the
// name, then formats and license class.
//
// The dot lettering is the Allternit dot font (same 10-unit grid, 8.5 dots,
// 1.5 radius as the A://TERNIT wordmark), copied from fw-cards.js `FONT`.

enum FwDotFont {
    static let glyphs: [Character: [String]] = [
        "A": ["..#..", ".#.#.", "#####", "#...#", "#...#"], "B": ["####.", "#...#", "####.", "#...#", "####."],
        "C": [".####", "#....", "#....", "#....", ".####"], "D": ["####.", "#...#", "#...#", "#...#", "####."],
        "E": ["#####", "#....", "####.", "#....", "#####"], "F": ["#####", "#....", "####.", "#....", "#...."],
        "G": [".####", "#....", "#.###", "#...#", ".###."], "H": ["#...#", "#...#", "#####", "#...#", "#...#"],
        "I": ["###", ".#.", ".#.", ".#.", "###"], "J": ["..###", "...#.", "...#.", "#..#.", ".##.."],
        "K": ["#...#", "#..#.", "###..", "#..#.", "#...#"], "L": ["#....", "#....", "#....", "#....", "#####"],
        "M": ["#...#", "##.##", "#.#.#", "#...#", "#...#"], "N": ["#...#", "##..#", "#.#.#", "#..##", "#...#"],
        "O": [".###.", "#...#", "#...#", "#...#", ".###."], "P": ["####.", "#...#", "####.", "#....", "#...."],
        "Q": [".###.", "#...#", "#.#.#", "#..#.", ".##.#"], "R": ["####.", "#...#", "####.", "#.#..", "#..##"],
        "S": [".####", "#....", ".###.", "....#", "####."], "T": ["#####", "..#..", "..#..", "..#..", "..#.."],
        "U": ["#...#", "#...#", "#...#", "#...#", ".###."], "V": ["#...#", "#...#", "#...#", ".#.#.", "..#.."],
        "W": ["#...#", "#...#", "#.#.#", "##.##", "#...#"], "X": ["#...#", ".#.#.", "..#..", ".#.#.", "#...#"],
        "Y": ["#...#", ".#.#.", "..#..", "..#..", "..#.."], "Z": ["#####", "...#.", "..#..", ".#...", "#####"],
        "0": [".###.", "#..##", "#.#.#", "##..#", ".###."], "1": [".#.", "##.", ".#.", ".#.", "###"],
        "2": ["####.", "....#", ".###.", "#....", "#####"], "3": ["####.", "....#", ".###.", "....#", "####."],
        "4": ["#..#.", "#..#.", "#####", "...#.", "...#."], "5": ["#####", "#....", "####.", "....#", "####."],
        "6": [".###.", "#....", "####.", "#...#", ".###."], "7": ["#####", "....#", "...#.", "..#..", "..#.."],
        "8": [".###.", "#...#", ".###.", "#...#", ".###."], "9": [".###.", "#...#", ".####", "....#", ".###."],
        ".": [".", ".", ".", ".", "#"], ":": [".", "#", ".", "#", "."], "/": ["..#", "..#", ".#.", "#..", "#.."],
        "-": ["...", "...", "###", "...", "..."], " ": ["..", "..", "..", "..", ".."],
    ]

    /// Dot cells for `text`, letters one empty column apart. Returns the
    /// lit cells and the width in columns (5 rows tall).
    static func layout(_ text: String) -> (cells: [(Int, Int)], cols: Int) {
        var cells: [(Int, Int)] = []
        var c = 0
        let chars = Array(text.uppercased())
        for (i, ch) in chars.enumerated() {
            let g = glyphs[ch] ?? glyphs[" "]!
            for (r, line) in g.enumerated() {
                for (x, mark) in line.enumerated() where mark == "#" { cells.append((c + x, r)) }
            }
            c += g[0].count + (i < chars.count - 1 ? 1 : 0)
        }
        return (cells, c)
    }
}

/// Dot lettering at a given height (the site's `mono(r, h)`).
struct FwDotWord: View {
    let text: String
    var height: CGFloat = 40

    var body: some View {
        let l = FwDotFont.layout(text)
        let cell = height / 5
        Canvas { ctx, _ in
            for (x, y) in l.cells {
                let r = CGRect(x: CGFloat(x) * cell + cell * 0.075, y: CGFloat(y) * cell + cell * 0.075,
                               width: cell * 0.85, height: cell * 0.85)
                ctx.fill(Path(roundedRect: r, cornerRadius: cell * 0.15), with: .foreground)
            }
        }
        .frame(width: CGFloat(l.cols) * cell, height: height)
        .accessibilityHidden(true)
    }
}

/// What the identity face shows, built from catalog rows.
struct FwFaceModel: Equatable {
    var id: String
    var name: String
    var family: String?
    var params: Int64?
    /// Bytes of the build shown (device data), never the whole repo.
    var sizeBytes: Double?
    var formats: [String]
    var licenseClass: String?
    var licenseId: String?
    var logo: URL?

    /// Two letters from the family ("qwen" → "QW"), else "FW" (FwCard.tsx `mono`).
    var mono: String {
        let letters = (family ?? "").filter { $0.isLetter || $0.isNumber }
        return letters.isEmpty ? "FW" : String(letters.prefix(2)).uppercased()
    }

    init(id: String, name: String, family: String? = nil, params: Int64? = nil, sizeBytes: Double? = nil,
         formats: [String] = [], licenseClass: String? = nil, licenseId: String? = nil, logo: URL? = nil) {
        self.id = id
        self.name = name
        self.family = family
        self.params = params
        self.sizeBytes = sizeBytes
        self.formats = formats
        self.licenseClass = licenseClass
        self.licenseId = licenseId
        self.logo = logo
    }

    init(summary: FreeweightsModelSummary?, row: FreeweightsCatalogRow?, id: String, sizeBytes: Double?, formats: [String]? = nil) {
        let fallbackFamily = summary?.section.map(FreeweightsFormat.section)
        self.init(id: id, name: row?.displayName ?? summary?.name ?? String(id.split(separator: "/").last ?? Substring(id)),
                  family: row?.family ?? fallbackFamily, params: row?.params, sizeBytes: sizeBytes,
                  formats: formats ?? row?.formats ?? [], licenseClass: row?.licenseClass,
                  licenseId: row?.licenseId ?? summary?.license, logo: summary?.publisherLogoURL)
    }
}

extension Color {
    /// Freeweights `--accent` (fw.css): #B83E1B light, #E8896C dark.
    static let fwAccent = Color(UIColor { $0.userInterfaceStyle == .dark
        ? UIColor(red: 0.910, green: 0.537, blue: 0.424, alpha: 1)
        : UIColor(red: 0.722, green: 0.243, blue: 0.106, alpha: 1) })
}

enum FwText {
    static let licenseNames = ["copyleft": "copyleft", "permissive": "permissive", "custom": "custom terms",
                               "non-commercial": "non-commercial", "gated": "gated", "territory": "territory-limited",
                               "unreviewed": "license not reviewed"]

    /// The site's `bytes()`: decimal units, 2 decimals under 10, 1 under 100.
    static func bytes(_ n: Double?) -> (value: String, unit: String) {
        guard let n, n.isFinite, n >= 0 else { return ("—", "") }
        let units = ["B", "KB", "MB", "GB", "TB"]
        var v = n, i = 0
        while v >= 1000, i < units.count - 1 { v /= 1000; i += 1 }
        let s = v >= 100 || i == 0 ? String(Int(v.rounded())) : v >= 10 ? String(format: "%.1f", v) : String(format: "%.2f", v)
        return (s, units[i])
    }

    static func bytesText(_ n: Double?) -> String {
        let b = bytes(n)
        return b.unit.isEmpty ? "size unknown" : "\(b.value) \(b.unit)"
    }

    /// "2.7 GB" / "640 MB" (FwCard.tsx `formatDeviceSize`).
    static func deviceSize(_ n: Double) -> String {
        let gb = n / 1e9
        if gb >= 1 { return gb >= 100 ? "\(Int(gb.rounded())) GB" : String(format: "%.1f GB", gb) }
        return "\(max(1, Int((n / 1e6).rounded()))) MB"
    }

    /// The site's `params()`: 4.2B, 751M, 1.2T.
    static func params(_ n: Int64?) -> String? {
        guard let n, n > 0 else { return nil }
        let d = Double(n)
        func trim(_ v: Double, _ digits: Int) -> String {
            var s = String(format: "%.\(digits)f", v)
            if s.contains(".") { while s.hasSuffix("0") { s.removeLast() }; if s.hasSuffix(".") { s.removeLast() } }
            return s
        }
        if d >= 1e12 { return trim(d / 1e12, 2) + "T" }
        if d >= 1e9 { return trim(d / 1e9, 1) + "B" }
        return trim(d / 1e6, 0) + "M"
    }
}

/// The identity face. `compact` is the two-column phone tile (the mockup's
/// `.iptile`: 24 pt size figure, 16 pt dot letters).
struct FwIdentityFace: View {
    let model: FwFaceModel
    var compact = false

    @ScaledMetric(relativeTo: .largeTitle) private var bigSize: CGFloat = 34
    @ScaledMetric(relativeTo: .largeTitle) private var smallSize: CGFloat = 24

    var body: some View {
        let size = FwText.bytes(model.sizeBytes)
        VStack(alignment: .leading, spacing: compact ? 3 : 4) {
            HStack(alignment: .center, spacing: 8) {
                Text(FwText.params(model.params).map { "\($0) params" } ?? (model.family ?? ""))
                    .font(.system(size: compact ? 10 : 11, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                Spacer(minLength: 0)
                if let logo = model.logo {
                    AsyncImage(url: logo) { phase in
                        if case .success(let image) = phase { image.resizable().scaledToFit() } else { Color.clear }
                    }
                    .frame(width: compact ? 18 : 24, height: compact ? 18 : 24)
                    .clipShape(Circle())
                    .accessibilityHidden(true)
                }
            }
            .frame(minHeight: compact ? 18 : 24)

            FwDotWord(text: model.mono, height: compact ? 16 : 40)
                .foregroundColor(Color("TextPrimary"))
                .padding(.vertical, compact ? 2 : 4)

            if size.unit.isEmpty {
                Text("Size not checked")
                    .font(.system(size: compact ? 12 : 15, weight: .semibold, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(minHeight: compact ? smallSize : bigSize, alignment: .bottomLeading)
            } else {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Text(size.value)
                        .font(.system(size: compact ? smallSize : bigSize, weight: .semibold, design: .monospaced))
                        .tracking(compact ? -0.8 : -1.3)
                        .monospacedDigit()
                    Text(size.unit)
                        .font(.system(size: compact ? 11 : 13, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                }
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
                .minimumScaleFactor(0.6)
            }

            Text(model.name)
                .font(.system(size: compact ? 12 : 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
                .truncationMode(.tail)
                .padding(.top, compact ? 2 : 4)

            VStack(spacing: compact ? 4 : 6) {
                Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
                HStack(spacing: 8) {
                    Text(model.formats.isEmpty ? "files" : model.formats.joined(separator: " "))
                    Spacer(minLength: 0)
                    Text(FwText.licenseNames[model.licenseClass ?? ""] ?? model.licenseClass ?? "see card")
                }
                .font(.system(size: compact ? 9.5 : 10.5, design: .monospaced))
                .foregroundColor(Color("TextTertiary"))
                .lineLimit(1)
            }
        }
        .padding(.horizontal, compact ? 9 : 13)
        .padding(.top, compact ? 8 : 11)
        .padding(.bottom, compact ? 9 : 13)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color("TextPrimary"), lineWidth: 1.5))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(accessibilityText)
    }

    private var accessibilityText: String {
        var parts = [model.name]
        if let p = FwText.params(model.params) { parts.append("\(p) parameters") }
        parts.append(model.sizeBytes.map { FwText.bytesText($0) } ?? "size not checked")
        if let lc = model.licenseClass { parts.append(FwText.licenseNames[lc] ?? lc) }
        return parts.joined(separator: ", ")
    }
}

// MARK: - Fit label (the one fit function's result, shown the same everywhere)

enum FitTone {
    static func color(_ k: FitKind) -> Color {
        switch k {
        case .ok: return Theme.statusSuccess
        case .slow: return Theme.statusWarning
        case .no: return Theme.statusError
        case .na, .unknown: return Color("TextTertiary")
        }
    }
}

/// "● Fits · 1.2 GB": a status dot beside readable text (status colors are
/// too light for text on the light theme).
struct FitLabel: View {
    let fit: FitResult
    var showSize = true
    var font: Font = .system(size: 11)

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(FitTone.color(fit.k)).frame(width: 7, height: 7)
            Text(text)
                .font(font)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(2)
        }
        .accessibilityElement(children: .combine)
    }

    private var text: String {
        if showSize, let q = fit.quant, fit.k != .na, fit.k != .unknown { return "\(fit.label) · \(FwText.deviceSize(q.size))" }
        return fit.label
    }
}
