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
 * The approved GIZZI CODE block wordmark (Allternit Assets/Brand/Gizzi/
 * wordmark/gizzi-code-wordmark.svg): A:// matrix letterforms, one-block
 * letter gap, three-block word gap, coral core in the G. One block per pixel
 * with half blocks, so it is three rows tall, matching the header mark.
 */
const WORDMARK_LETTERS: Record<string, string[]> = {
  G: ['.XXX.', 'X....', 'X.CXX', 'X...X', '.XXX.'],
  I: ['XXX', '.X.', '.X.', '.X.', 'XXX'],
  Z: ['XXXXX', '...X.', '..X..', '.X...', 'XXXXX'],
  C: ['.XXXX', 'X....', 'X....', 'X....', '.XXXX'],
  O: ['.XXX.', 'X...X', 'X...X', 'X...X', '.XXX.'],
  D: ['XXXX.', 'X...X', 'X...X', 'X...X', 'XXXX.'],
  E: ['XXXXX', 'X....', 'XXXX.', 'X....', 'XXXXX'],
}
const wordPixels = (word: string) =>
  [0, 1, 2, 3, 4].map(r => word.split('').map(ch => WORDMARK_LETTERS[ch]![r]).join('.'))
// Blank top pixel row: the letters line up with the mark's head, 6 rows = 3 cells.
const WORDMARK_PIXELS = ['', ...[0, 1, 2, 3, 4].map(r => wordPixels('GIZZI')[r] + '...' + wordPixels('CODE')[r])].map(
  (row, _, all) => row.padEnd(all[1]!.length, '.'),
)

export const WORDMARK_WIDTH = WORDMARK_PIXELS[0]!.length

/** GIZZI CODE wordmark rows: ink in the theme text color, coral core. */
export function wordmarkRows(): ArtRow[] {
  return renderPixelArt(WORDMARK_PIXELS, p => (p === 'X' ? 'text' : p === 'C' ? CORAL : undefined))
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
