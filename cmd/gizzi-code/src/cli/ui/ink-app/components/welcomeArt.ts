/**
 * Gizzi pixel-art support shared by the pet sprite: theme color keys
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
 * Text-only header mark, for terminals that can't show inline images: the
 * official Gizzi mascot (Brand/Gizzi/static/gizzi-mascot.svg) in its own
 * colors, 8 columns by 4 rows. A sand body with the face panel a shade
 * darker keeps one solid silhouette at this size, the way Claude Code's
 * mascot reads. Beacon, ear pods, face panel, eyes, coral nose, hands, four
 * legs. Quadrant blocks give 2x2 pixels per cell; a pixel is half a cell
 * wide and half a cell tall, so the eyes are two pixels wide to stay square.
 */
const TEXT_MARK_PIXELS = [
  '.......BB.......',
  '..TTTT....TTTT..',
  '..TTTTTTTTTTTT..',
  '.TTVVVVVVVVVVTT.',
  'TTTVEEVVVVEEVTTT',
  '.TTVVVVCCVVVVTT.',
  '..TTTTTTTTTTTT..',
  '..TT.TT..TT.TT..',
]

export function textMarkRows(): ArtRow[] {
  return renderQuadrantArt(TEXT_MARK_PIXELS, p =>
    p === 'B' || p === 'C' ? CORAL : p === 'T' ? SAND : p === 'V' ? VISOR : p === 'E' ? EYE : undefined,
  )
}

// Quadrant block for each 2x2 pattern, indexed by bits
// 1 = top-left, 2 = top-right, 4 = bottom-left, 8 = bottom-right.
const QUADRANTS = ' ▘▝▀▖▌▞▛▗▚▐▜▄▙▟█'

/**
 * Four pixels per terminal cell with quadrant blocks. A cell holds at most
 * two colors: the second one becomes the background behind the glyph.
 */
export function renderQuadrantArt(
  pixels: string[],
  colorOf: (p: string) => string | undefined,
): ArtRow[] {
  const width = pixels[0]!.length
  const rows: ArtRow[] = []
  for (let r = 0; r < pixels.length; r += 2) {
    const cells: ArtSegment[] = []
    for (let c = 0; c < width; c += 2) {
      const quad = [
        colorOf(pixels[r]![c]!),
        colorOf(pixels[r]![c + 1]!),
        colorOf(pixels[r + 1]?.[c] ?? '.'),
        colorOf(pixels[r + 1]?.[c + 1] ?? '.'),
      ]
      const fg = quad.find(Boolean)
      if (!fg) {
        cells.push([' ', ''])
        continue
      }
      const bg = quad.find(q => q && q !== fg)
      let bits = 0
      quad.forEach((q, i) => {
        if (q === fg) bits |= 1 << i
      })
      cells.push(bg ? [QUADRANTS[bits]!, fg, bg] : [QUADRANTS[bits]!, fg])
    }
    rows.push(mergeRuns(cells))
  }
  return rows
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
