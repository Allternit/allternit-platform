/**
 * Gizzi pixel art shared by the header and the pet sprite: theme color keys
 * (gizziSand / gizziVisor / gizziEye / gizzi coral) and a half-block renderer.
 */

export const CORAL = 'gizzi'
export const SAND = 'gizziSand'
export const VISOR = 'gizziVisor'
export const EYE = 'gizziEye'

/** [text, foreground, background?] */
export type ArtSegment = [text: string, color: string, bg?: string]
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
  '...TTTTT..TTTTT...',
  '..................',
  '...TTTTTTTTTTTT...',
  '..TTVVVVVVVVVVTT..',
  '..TVVEEVVVVEEVVT..',
  'TTTVVEEVVVVEEVVTTT',
  '.TTVVVVVBBVVVVVTT.',
  '..TVVVVBVVBVVVVT..',
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
    if (last && last[1] === cell[1] && last[2] === cell[2]) last[0] += cell[0]
    else row.push([...cell] as ArtSegment)
  }
  return row
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
    rows.push(mergeRuns(cells))
  }
  return rows
}

/**
 * Pixel fonts for the GIZZI CODE dot-matrix wordmark, 5 pixels tall.
 * FONT35 letters are 3 wide, FONT45 letters 4 wide (G, Z, C, O, D, E).
 */
export const FONT35: Record<string, string[]> = {
  G: ['.##', '#..', '#.#', '#.#', '.##'], I: ['###', '.#.', '.#.', '.#.', '###'],
  Z: ['###', '..#', '.#.', '#..', '###'], C: ['.##', '#..', '#..', '#..', '.##'],
  O: ['.#.', '#.#', '#.#', '#.#', '.#.'], D: ['##.', '#.#', '#.#', '#.#', '##.'],
  E: ['###', '#..', '##.', '#..', '###'], ' ': ['..', '..', '..', '..', '..'],
}
export const FONT45: Record<string, string[]> = {
  G: ['.###', '#...', '#.##', '#..#', '.###'], I: ['###', '.#.', '.#.', '.#.', '###'],
  Z: ['####', '..#.', '.#..', '#...', '####'], C: ['.###', '#...', '#...', '#...', '.###'],
  O: ['.##.', '#..#', '#..#', '#..#', '.##.'], D: ['###.', '#..#', '#..#', '#..#', '###.'],
  E: ['####', '#...', '###.', '#...', '####'], ' ': ['..', '..', '..', '..', '..'],
}

/** Eoj's wordmark pick (b2, 2026-11-09): the font, whether the first G is coral, and the braille row the version sits on. */
export const WORDMARK_FONT = FONT35
export const WORDMARK_CORAL_G = true
export const WORDMARK_VERSION_ROW = 0

/** A word as a pixel grid, one blank column between letters. */
export function wordGrid(word: string, font: Record<string, string[]>): string[] {
  return Array.from({ length: 5 }, (_, r) =>
    [...word].map(ch => font[ch]![r]!).join('.'),
  )
}

// Braille dot bits for [dx, dy] inside a 2x4 cell.
const BRAILLE_BITS: Array<[number, number, number]> = [
  [0, 0, 0x01], [0, 1, 0x02], [0, 2, 0x04], [1, 0, 0x08],
  [1, 1, 0x10], [1, 2, 0x20], [0, 3, 0x40], [1, 3, 0x80],
]

/** Pixels as braille dots (U+2800-U+28FF): 2x4 pixels per one-column cell. */
export function brailleRows(grid: string[]): string[] {
  const w2 = Math.ceil(grid[0]!.length / 2) * 2
  const h4 = Math.ceil(grid.length / 4) * 4
  const g = Array.from({ length: h4 }, (_, r) => (grid[r] ?? '').padEnd(w2, '.'))
  const out: string[] = []
  for (let by = 0; by < h4; by += 4) {
    let line = ''
    for (let bx = 0; bx < w2; bx += 2) {
      let v = 0
      for (const [dx, dy, bit] of BRAILLE_BITS) if (g[by + dy]![bx + dx] !== '.') v |= bit
      line += v ? String.fromCharCode(0x2800 + v) : ' '
    }
    out.push(line)
  }
  return out
}

/**
 * The GIZZI CODE wordmark as 2 rows of braille. With `coralG` the first G
 * (and the gap after it) is its own run in coral; the rest takes `ink`.
 */
export function wordmarkRows(
  font: Record<string, string[]> = WORDMARK_FONT,
  coralG: boolean = WORDMARK_CORAL_G,
  ink = 'text',
): ArtRow[] {
  const grid = wordGrid('GIZZI CODE', font)
  if (!coralG) return brailleRows(grid).map(t => [[t, ink]])
  const gw = font.G![0]!.length + 1
  const g = brailleRows(grid.map(r => r.slice(0, gw)))
  const rest = brailleRows(grid.map(r => r.slice(gw)))
  return g.map((t, i) => [[t, CORAL], [rest[i]!, ink]])
}
