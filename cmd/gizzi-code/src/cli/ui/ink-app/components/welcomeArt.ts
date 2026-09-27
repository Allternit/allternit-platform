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
  const rows: ArtRow[] = []
  for (let r = 0; r < GIZZI_HEIGHT; r++) {
    const top = GIZZI_PIXELS[r * 2]!
    const bottom = GIZZI_PIXELS[r * 2 + 1]!
    const cells: ArtSegment[] = []
    for (let c = 0; c < GIZZI_WIDTH; c++) {
      if (r === MARK_ROW && c >= MARK_COL && c < MARK_COL + MARK.length) {
        cells.push([MARK[c - MARK_COL]!, CORAL, VISOR])
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

/** 5-row block wordmark, one string per row, letters joined by one space. */
const LETTERS: Record<string, string[]> = {
  G: [
    ' ████ ',
    '██    ',
    '██ ███',
    '██  ██',
    ' ████ ',
  ],
  I: [
    '██████',
    '  ██  ',
    '  ██  ',
    '  ██  ',
    '██████',
  ],
  Z: [
    '██████',
    '   ██ ',
    '  ██  ',
    ' ██   ',
    '██████',
  ],
}

export const WORDMARK_WORD = 'GIZZI'

export const WORDMARK_ROWS: string[] = [0, 1, 2, 3, 4].map(row =>
  WORDMARK_WORD.split('')
    .map(ch => LETTERS[ch][row])
    .join(' '),
)

export const WORDMARK_WIDTH = WORDMARK_ROWS[0].length
