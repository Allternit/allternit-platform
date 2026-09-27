/**
 * Startup-screen art: the Gizzi mascot and the GIZZI block wordmark, as
 * data, so WelcomeBox can animate them (beacon pulse, eye blink, shimmer
 * sweep) without hardcoding frames inline.
 *
 * The mascot is the brand Gizzi (Allternit Assets/Brand/Gizzi/static/
 * gizzi-mascot.svg) drawn on a 20x20 pixel grid, two pixels per terminal
 * cell with half blocks: coral beacon, ear pods, sand body with side
 * hands, darker face panel with square eyes, coral "A://" mark, four legs.
 * Colors are theme keys (gizziSand / gizziVisor / gizziEye / gizzi); the
 * beacon color is passed in because it animates.
 */

export const CORAL = 'gizzi'
export const SAND = 'gizziSand'
export const VISOR = 'gizziVisor'
export const EYE = 'gizziEye'

/** [text, foreground, background?] */
export type ArtSegment = [text: string, color: string, bg?: string]
export type ArtRow = ArtSegment[]

// Pixel map: . empty, B beacon, T sand, V face panel, E eye.
const GIZZI_PIXELS = [
  '.........BB.........',
  '....................',
  '....TTTT....TTTT....',
  '....................',
  '....TTTTTTTTTTTT....',
  '...TTTTTTTTTTTTTT...',
  '...TTVVVVVVVVVVTT...',
  '...TVVVVVVVVVVVVT...',
  '...TVVEEVVVVEEVVT...',
  '..TTVVEEVVVVEEVVTT..',
  '.TTTVVVVVVVVVVVVTTT.',
  '..TTVVVVVVVVVVVVTT..',
  '..TTVVVVVVVVVVVVTT..',
  '...TTVVVVVVVVVVTT...',
  '....TTTTTTTTTTTT....',
  '....TTTTTTTTTTTT....',
  '.....TTTTTTTTTT.....',
  '....TT.TT..TT.TT....',
  '....TT.TT..TT.TT....',
  '....TT.TT..TT.TT....',
]

export const GIZZI_WIDTH = GIZZI_PIXELS[0]!.length
export const GIZZI_HEIGHT = GIZZI_PIXELS.length / 2

/** Cell row of the eyes (pixel rows 8-9) and of the "A://" mark. */
export const EYE_ROW = 4
const MARK_ROW = 5
const MARK = 'A://'
const MARK_COL = (GIZZI_WIDTH - MARK.length) / 2

/**
 * Gizzi rows with animatable slots: the beacon (row 0) takes `beaconColor`;
 * while `blinking` the eyes close into the face panel.
 */
export function gizziRows({
  beaconColor,
  blinking,
}: {
  beaconColor: string
  blinking: boolean
}): ArtRow[] {
  const colorOf = (p: string): string | undefined =>
    p === 'B' ? beaconColor : p === 'T' ? SAND : p === 'V' ? VISOR : p === 'E' ? (blinking ? VISOR : EYE) : undefined
  return renderPixelArt(GIZZI_PIXELS, colorOf, { row: MARK_ROW, col: MARK_COL, text: MARK })
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

/**
 * The approved GIZZI block wordmark (Allternit Assets/Brand/Gizzi/wordmark/),
 * same matrix system as A://TERNIT: 5x5 block letters, one block column gap,
 * coral core in the G crossbar. Each block is one ■ followed by a space so the
 * blocks read square with visible gaps in a 1:2 terminal cell.
 */
const LETTER_GRIDS: Record<string, string[]> = {
  G: ['.XXX.', 'X....', 'X.CXX', 'X...X', '.XXX.'],
  I: ['XXX', '.X.', '.X.', '.X.', 'XXX'],
  Z: ['XXXXX', '...X.', '..X..', '.X...', 'XXXXX'],
}

export const WORDMARK_WORD = 'GIZZI'
export const WORDMARK_BLOCK = '■'

const GRID_ROWS: string[] = [0, 1, 2, 3, 4].map(row =>
  WORDMARK_WORD.split('')
    .map(ch => LETTER_GRIDS[ch]![row])
    .join('.'),
)

/** Rendered rows: block → "■ ", gap → "  ", trailing space trimmed to a fixed width. */
export const WORDMARK_ROWS: string[] = GRID_ROWS.map(row =>
  row
    .split('')
    .map(ch => (ch === '.' ? '  ' : `${WORDMARK_BLOCK} `))
    .join('')
    .slice(0, row.length * 2 - 1),
)

export const WORDMARK_WIDTH = WORDMARK_ROWS[0]!.length

/** Row and string index of the coral core block. */
export const WORDMARK_CORE = (() => {
  const row = GRID_ROWS.findIndex(r => r.includes('C'))
  return { row, col: GRID_ROWS[row]!.indexOf('C') * 2 }
})()
