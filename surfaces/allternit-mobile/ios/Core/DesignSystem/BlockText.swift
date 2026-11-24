import SwiftUI

/// Text drawn in the Allternit pixel block type (web block-type.ts), from the
/// generated `BlockGlyphs` table. `height` is the height of one line of
/// letters (5 rows). With `maxWidth` the text wraps at spaces; lines are
/// `height * 0.36` apart. Reads as plain `text` to VoiceOver.
struct BlockText: View {
    let text: String
    var height: CGFloat = 22
    var color: Color = Color("TextPrimary")
    var maxWidth: CGFloat?
    var isHeader = false

    init(_ text: String, height: CGFloat = 22, color: Color = Color("TextPrimary"),
         maxWidth: CGFloat? = nil, isHeader: Bool = false) {
        self.text = text
        self.height = height
        self.color = color
        self.maxWidth = maxWidth
        self.isHeader = isHeader
    }

    /// Fraction of `height` between two lines.
    nonisolated static let lineGap: CGFloat = 0.36

    /// One glyph position in a laid-out string. `cell` is the column and row
    /// inside the string's grid, `letter` counts inked letters in order (the
    /// docking title pairs big and small letters by this index).
    struct Cell {
        let col: Int
        let row: Int
        let letter: Int
    }

    /// Cells and column count for `text` on one row (no wrapping). The column
    /// count is the width of the string in grid columns, without the trailing
    /// gap. Characters with no glyph are skipped.
    nonisolated static func cells(for text: String) -> (cols: Int, cells: [Cell]) {
        var out: [Cell] = []
        var col = 0
        var letter = 0
        for ch in text.uppercased() {
            if ch == " " { col += BlockGlyphs.space; continue }
            if let glyph = BlockGlyphs.glyphs[ch] {
                out += glyph.map { Cell(col: $0.0 + col, row: $0.1, letter: letter) }
                col += BlockGlyphs.pitch
                letter += 1
            } else if let mark = BlockGlyphs.punct[ch] {
                out += mark.cells.map { Cell(col: $0.0 + col, row: $0.1, letter: letter) }
                col += mark.w + 1
                letter += 1
            }
        }
        return (max(0, col - 1), out)
    }

    /// Column count of `text` on one row.
    nonisolated static func columns(of text: String) -> Int { cells(for: text).cols }

    /// Splits at spaces so no line is wider than `maxWidth` points when drawn
    /// at `height` (a single longer word stays on its own line).
    nonisolated static func lines(for text: String, height: CGFloat, maxWidth: CGFloat?) -> [String] {
        guard let maxWidth else { return [text] }
        var lines: [String] = []
        var cur = ""
        for word in text.split(separator: " ", omittingEmptySubsequences: true) {
            let next = cur.isEmpty ? String(word) : cur + " " + word
            if !cur.isEmpty, CGFloat(columns(of: next)) * height / CGFloat(BlockGlyphs.rows) > maxWidth {
                lines.append(cur)
                cur = String(word)
            } else {
                cur = next
            }
        }
        lines.append(cur)
        return lines
    }

    private var laidOut: (lines: [(cols: Int, cells: [Cell])], size: CGSize) {
        let unit = height / CGFloat(BlockGlyphs.rows)
        let lines = Self.lines(for: text, height: height, maxWidth: maxWidth).map { Self.cells(for: $0) }
        let width = CGFloat(lines.map(\.cols).max() ?? 0) * unit
        let gap = height * Self.lineGap
        let total = CGFloat(lines.count) * height + CGFloat(max(0, lines.count - 1)) * gap
        return (lines, CGSize(width: width, height: total))
    }

    var body: some View {
        let layout = laidOut
        let unit = height / CGFloat(BlockGlyphs.rows)
        let inset = unit * CGFloat((BlockGlyphs.cell - BlockGlyphs.block) / 2 / BlockGlyphs.cell)
        let block = unit * CGFloat(BlockGlyphs.block / BlockGlyphs.cell)
        let radius = unit * CGFloat(BlockGlyphs.radius / BlockGlyphs.cell)
        let gap = height * Self.lineGap
        Canvas { context, _ in
            var inked = Path()
            for (index, line) in layout.lines.enumerated() {
                let top = CGFloat(index) * (height + gap)
                for c in line.cells {
                    inked.addRoundedRect(
                        in: CGRect(x: CGFloat(c.col) * unit + inset, y: top + CGFloat(c.row) * unit + inset,
                                   width: block, height: block),
                        cornerSize: CGSize(width: radius, height: radius))
                }
            }
            context.fill(inked, with: .color(color))
        }
        .frame(width: layout.size.width, height: layout.size.height)
        .accessibilityElement()
        .accessibilityLabel(text)
        .accessibilityAddTraits(isHeader ? .isHeader : [])
    }
}
