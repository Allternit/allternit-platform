/**
 * Gizzi pixel art shared by the header and the pet sprite: theme color keys
 * (gizziSand / gizziVisor / gizziEye / gizzi coral) and a half-block renderer.
 */

export const CORAL = 'gizzi'
export const SAND = 'gizziSand'
export const VISOR = 'gizziVisor'
export const EYE = 'gizziEye'

/** [text, foreground, background?, inverse?] */
export type ArtSegment = [text: string, color: string, bg?: string, inverse?: boolean]
export type ArtRow = ArtSegment[]

/**
 * Header mascot (Allternit Assets/Brand/Gizzi/gizzi-mascot-transparent.png)
 * as half blocks: two pixel rows per cell, so every feature sits on whole
 * cells (▀ = top pixel as fg, bottom pixel as bg). Beacon, ear pods, chamfered
 * head, face panel, square eyes, coral A nose, side hands, body, four legs.
 * B coral, T sand, V face panel, E eye. 18 columns x 8 rows.
 */
export const MASCOT_MID = [
  '........BB........',
  '..................',
  '...TTTTE..ETTTT...',
  '..................',
  '...TTTTTTTTTTTT...',
  '..TTVVVVVVVVVVTT..',
  '..TVVEEVVVVEEVVT..',
  '.TTVVEEVVVVEEVVTT.',
  '.TTVVVVVBBVVVVVTT.',
  '..TVVVVBBBBVVVVT..',
  '..TTVVVVVVVVVVTT..',
  '...TTTTTTTTTTTT...',
  '...TTTTTTTTTTTT...',
  '....TTTTTTTTTT....',
  '...TT.TT..TT.TT...',
  '...TT.TT..TT.TT...',
]

/** Small terminals: 10 columns x 3 rows. */
export const MASCOT_TINY = [
  '....BB....',
  '.TTT..TTT.',
  '.TTTTTTTT.',
  'TTVEVVEVTT',
  '.TVVBBVVT.',
  '.T.T..T.T.',
]

export type MascotSize = 'mid' | 'tiny'

/**
 * Mascot rows. `coral` colors the beacon and nose (the pulse swaps in a
 * lighter coral); `blinking` closes the eyes into the face panel.
 */
export function mascotRows(
  size: MascotSize = 'mid',
  opts: { coral?: string; blinking?: boolean } = {},
): ArtRow[] {
  const grid = size === 'tiny' ? MASCOT_TINY : MASCOT_MID
  const coral = opts.coral ?? CORAL
  return renderPixelArt(opts.blinking ? grid.map(r => r.replace(/E/g, 'V')) : grid, p =>
    p === 'B' ? coral : p === 'T' ? SAND : p === 'V' ? VISOR : p === 'E' ? EYE : undefined,
  )
}

function mergeRuns(cells: ArtSegment[]): ArtRow {
  const row: ArtRow = []
  for (const cell of cells) {
    const last = row[row.length - 1]
    if (last && last[1] === cell[1] && last[2] === cell[2] && last[3] === cell[3]) last[0] += cell[0]
    else row.push([...cell] as ArtSegment)
  }
  return row
}

/**
 * One cell from its top and bottom pixel colors, gap-free: ▀ starts below the
 * cell top in some fonts (SF Mono), so a split cell is drawn with ▄ (reaches
 * the cell bottom) and the top color as the cell background; a solid cell is a
 * space on a background. A top pixel over an empty bottom is a reverse-video ▄
 * with the top color as foreground: reverse fills the whole cell with that
 * color and draws the glyph in the terminal's own background, so no terminal
 * background color needs to be known.
 */
export function pixelCell(t: string | undefined, b: string | undefined): ArtSegment {
  if (!t && !b) return [' ', '']
  if (t && b) return t === b ? [' ', '', t] : ['▄', b, t]
  if (b) return ['▄', b]
  return ['▄', t!, undefined, true]
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
      cells.push(pixelCell(t, b))
    }
    rows.push(mergeRuns(cells))
  }
  return rows
}

/**
 * GIZZI CODE wordmark "R2": 4 x 4 pixel letters condensed from the real Gizzi
 * Code wordmark. 'C' in the G is the coral core block.
 */
export const WORDMARK_LETTERS: Record<string, string[]> = {
  G: ['.###', '#...', '#.C#', '.###'], I: ['###', '.#.', '.#.', '###'],
  Z: ['####', '..#.', '.#..', '####'], C: ['.###', '#...', '#...', '.###'],
  O: ['.##.', '#..#', '#..#', '.##.'], D: ['###.', '#..#', '#..#', '###.'],
  E: ['####', '#...', '###.', '####'],
}

/** The version sits after the wordmark on its one row. */
export const WORDMARK_VERSION_ROW = 0

/** A word as a 4-row pixel grid: one empty column between letters, two between words. */
export function wordGrid(text: string, font: Record<string, string[]> = WORDMARK_LETTERS): string[] {
  const rows = ['', '', '', '']
  for (let i = 0; i < text.length; i++) {
    const ch = text[i]!
    const next = text[i + 1]
    for (let r = 0; r < 4; r++) {
      if (ch === ' ') rows[r] += '..'
      else rows[r] += font[ch]![r]! + (next && next !== ' ' ? '.' : '')
    }
  }
  return rows
}

// Braille dot bit for [dy][dx] inside a 2x4 cell.
const BRAILLE_BIT = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]]

/** One row of braille (1 dot per pixel); a cell holding a 'C' pixel is coral, the rest `ink`. */
export function brailleWordRow(grid: string[], ink = 'text'): ArtRow {
  const width = Math.ceil(grid[0]!.length / 2) * 2
  const cells: ArtSegment[] = []
  for (let cx = 0; cx < width; cx += 2) {
    let bits = 0
    let coral = false
    for (let dy = 0; dy < 4; dy++) {
      for (let dx = 0; dx < 2; dx++) {
        const p = grid[dy]?.[cx + dx]
        if (p && p !== '.') {
          bits |= BRAILLE_BIT[dy]![dx]!
          if (p === 'C') coral = true
        }
      }
    }
    cells.push(bits ? [String.fromCharCode(0x2800 + bits), coral ? CORAL : ink] : [' ', ''])
  }
  return mergeRuns(cells)
}

/** The GIZZI CODE wordmark: one braille row, no seam. */
export function wordmarkRows(ink = 'text'): ArtRow[] {
  return [brailleWordRow(wordGrid('GIZZI CODE'), ink)]
}
