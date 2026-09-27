/**
 * Gizzi pixel-art support shared by the buddy sprite: theme color keys
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
