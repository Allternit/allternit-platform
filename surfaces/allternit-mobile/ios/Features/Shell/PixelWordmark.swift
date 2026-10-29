import SwiftUI

/// The pixel A://TERNIT wordmark with an optional pixel-letter suffix on the
/// same grid (web AProtocolWordmark): the A:// mark in cols 0–14, letters at
/// pitch 6 from col 16, 10-unit cells with 8.5 blocks and 1.5 corner radius,
/// the A core block in coral. Ink follows the text colour.
struct PixelWordmark: View {
    var height: CGFloat = 16
    var suffix: String = ""

    private static let cell: CGFloat = 10
    private static let block: CGFloat = 8.5
    private static let rows: CGFloat = 5
    private static let ternitStart = 16
    private static let pitch = 6
    private static let spaceCols = 3

    private static let markCells: [(Int, Int)] = [
        (2, 0), (1, 1), (3, 1), (0, 2), (1, 2), (3, 2), (4, 2), (0, 3), (4, 3), (0, 4), (4, 4),
        (6, 1), (6, 3),
        (8, 3), (8, 4), (9, 2), (10, 0), (10, 1),
        (12, 3), (12, 4), (13, 2), (14, 0), (14, 1),
    ]
    private static let coreCell = (2, 2)

    private static let glyphs: [Character: [(Int, Int)]] = [
        "T": [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (2, 1), (2, 2), (2, 3), (2, 4)],
        "E": [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (0, 1), (0, 2), (1, 2), (2, 2), (3, 2), (0, 3), (0, 4), (1, 4), (2, 4), (3, 4), (4, 4)],
        "R": [(0, 0), (1, 0), (2, 0), (3, 0), (0, 1), (4, 1), (0, 2), (1, 2), (2, 2), (3, 2), (0, 3), (2, 3), (0, 4), (3, 4), (4, 4)],
        "N": [(0, 0), (0, 1), (1, 1), (0, 2), (2, 2), (0, 3), (3, 3), (0, 4), (4, 0), (4, 1), (4, 2), (4, 3), (4, 4)],
        "I": [(1, 0), (2, 0), (3, 0), (2, 1), (2, 2), (2, 3), (1, 4), (2, 4), (3, 4)],
        "S": [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (0, 1), (0, 2), (1, 2), (2, 2), (3, 2), (4, 2), (4, 3), (0, 4), (1, 4), (2, 4), (3, 4), (4, 4)],
        "U": [(0, 0), (0, 1), (0, 2), (0, 3), (4, 0), (4, 1), (4, 2), (4, 3), (0, 4), (1, 4), (2, 4), (3, 4), (4, 4)],
        "D": [(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 0), (2, 0), (3, 0), (4, 1), (4, 2), (4, 3), (1, 4), (2, 4), (3, 4)],
        "O": [(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (4, 0), (4, 1), (4, 2), (4, 3), (4, 4), (1, 0), (2, 0), (3, 0), (1, 4), (2, 4), (3, 4)],
        "L": [(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 4), (2, 4), (3, 4), (4, 4)],
        "A": [(2, 0), (1, 1), (3, 1), (0, 2), (1, 2), (2, 2), (3, 2), (4, 2), (0, 3), (4, 3), (0, 4), (4, 4)],
        "B": [(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 0), (2, 0), (3, 0), (4, 1), (1, 2), (2, 2), (3, 2), (4, 3), (1, 4), (2, 4), (3, 4)],
        "F": [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (0, 1), (0, 2), (1, 2), (2, 2), (3, 2), (0, 3), (0, 4)],
        "C": [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 4), (2, 4), (3, 4), (4, 4)],
        "G": [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (0, 1), (0, 2), (1, 2), (2, 2), (3, 2), (4, 2), (0, 3), (4, 3), (0, 4), (1, 4), (2, 4), (3, 4), (4, 4)],
    ]

    /// Every inked cell (letters placed at their origin columns) and the
    /// total column count, as the web's layout().
    private static func layout(_ word: String) -> (cells: [(Int, Int)], cols: Int) {
        var cells: [(Int, Int)] = []
        var col = ternitStart
        for ch in word.uppercased() {
            if ch == " " { col += spaceCols; continue }
            if let glyph = glyphs[ch] { cells += glyph.map { ($0.0 + col, $0.1) } }
            col += pitch
        }
        return (cells, col - 1)
    }

    var body: some View {
        let (letters, cols) = Self.layout("TERNIT \(suffix)".trimmingCharacters(in: .whitespaces))
        let width = height * CGFloat(cols) / Self.rows
        Canvas { context, size in
            let scale = size.height / (Self.rows * Self.cell)
            let off = (Self.cell - Self.block) / 2
            func rect(_ c: (Int, Int)) -> Path {
                Path(roundedRect: CGRect(x: (CGFloat(c.0) * Self.cell + off) * scale,
                                         y: (CGFloat(c.1) * Self.cell + off) * scale,
                                         width: Self.block * scale, height: Self.block * scale),
                     cornerRadius: 1.5 * scale)
            }
            var ink = Path()
            for c in Self.markCells + letters { ink.addPath(rect(c)) }
            context.fill(ink, with: .color(Color("TextPrimary")))
            context.fill(rect(Self.coreCell), with: .color(Color(red: 0xD9 / 255, green: 0x77 / 255, blue: 0x57 / 255)))
        }
        .frame(width: width, height: height)
        .accessibilityElement()
        .accessibilityLabel("Allternit \(suffix)".trimmingCharacters(in: .whitespaces))
    }
}
