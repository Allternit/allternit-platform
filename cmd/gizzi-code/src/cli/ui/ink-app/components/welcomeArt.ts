/**
 * Startup header art: the Gizzi mark (Allternit Assets/Brand/Gizzi/mark,
 * compact form) drawn small, three terminal rows tall, beside inline
 * "Gizzi Code vX" text. Kept deliberately unobtrusive, like Claude Code's
 * header glyph. Colors are theme keys (gizziSand / gizziVisor / gizziEye /
 * gizzi coral).
 */

export const CORAL = 'gizzi'
export const SAND = 'gizziSand'
export const VISOR = 'gizziVisor'
export const EYE = 'gizziEye'

/** [text, foreground, background?] */
export type ArtSegment = [text: string, color: string, bg?: string]
export type ArtRow = ArtSegment[]

// Pixel map (two pixel rows per cell): . empty, B beacon, T sand, V face
// panel, E eye, C coral nose. Beacon, ear pods, head with side hands, face
// panel, eyes, nose.
const HEADER_PIXELS = [
  '.....BB.....',
  '..TTT..TTT..',
  '.TTTTTTTTTT.',
  'TTVEVVVVEVTT',
  '.TVVVVCVVVT.',
  '..TTTTTTTT..',
]

export const HEADER_MARK_WIDTH = HEADER_PIXELS[0]!.length
export const HEADER_MARK_HEIGHT = HEADER_PIXELS.length / 2

/** Header mark rows; while `blinking` the eyes close into the face panel. */
export function headerMarkRows({ blinking }: { blinking: boolean }): ArtRow[] {
  const colorOf = (p: string): string | undefined =>
    p === 'B' || p === 'C' ? CORAL : p === 'T' ? SAND : p === 'V' ? VISOR : p === 'E' ? (blinking ? VISOR : EYE) : undefined
  return renderPixelArt(HEADER_PIXELS, colorOf)
}

/**
 * Two pixel rows per terminal cell with half blocks. `colorOf` maps a pixel
 * char to a theme color (undefined = empty). An optional text `mark` is
 * drawn bold coral on the face panel at a cell row/column.
 */
export function renderPixelArt(
  pixels: string[],
  colorOf: (p: string) => string | undefined,
  mark?: { row: number; col: number; text: string },
): ArtRow[] {
  const width = pixels[0]!.length
  const rows: ArtRow[] = []
  for (let r = 0; r < pixels.length / 2; r++) {
    const top = pixels[r * 2]!
    const bottom = pixels[r * 2 + 1]!
    const cells: ArtSegment[] = []
    for (let c = 0; c < width; c++) {
      if (mark && r === mark.row && c >= mark.col && c < mark.col + mark.text.length) {
        cells.push([mark.text[c - mark.col]!, CORAL, VISOR])
        continue
      }
      const t = colorOf(top[c]!)
      const b = colorOf(bottom[c]!)
      if (!t && !b) cells.push([' ', ''])
      else if (t && b) cells.push(t === b ? ['█', t] : ['▀', t, b])
      else if (t) cells.push(['▀', t!])
      else cells.push(['▄', b!])
    }
    // Merge runs with the same styling.
    const row: ArtRow = []
    for (const cell of cells) {
      const last = row[row.length - 1]
      if (last && last[1] === cell[1] && last[2] === cell[2]) last[0] += cell[0]
      else row.push([...cell] as ArtSegment)
    }
    rows.push(row)
  }
  return rows
}
