import chalk from "@/shared/util/chalk"
import { mascotRows, wordmarkRows, type ArtRow } from "@/cli/ui/ink-app/components/welcomeArt"
import { getTheme } from "@/cli/ui/ink-app/utils/theme"

export const marks = "·•◦"

/** Parse a theme color ('rgb(r,g,b)'); ansi theme keys have no rgb and return undefined. */
function rgbOf(color: string | undefined): [number, number, number] | undefined {
  const m = color?.match(/^rgb\((\d+),\s*(\d+),\s*(\d+)\)$/)
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : undefined
}

/**
 * One art row as ANSI: 24-bit when the terminal supports it, else the chalk
 * shim's nearest-256 down-sampling. Plain text when color is off.
 */
export function ansiArtRow(row: ArtRow): string {
  const theme = getTheme("dark") as unknown as Record<string, string>
  return row
    .map(([text, fg, bg, inverse]) => {
      let s: ReturnType<typeof chalk.rgb> | typeof chalk = chalk
      const f = fg ? rgbOf(theme[fg]) : undefined
      const b = bg ? rgbOf(theme[bg]) : undefined
      if (f) s = s.rgb(f[0], f[1], f[2])
      if (b) s = s.bgRgb(b[0], b[1], b[2])
      const out = s(text)
      return inverse && out !== text ? `\x1b[7m${out}\x1b[27m` : out
    })
    .join("")
}

/** The `--help` banner: the approved mascot (18 x 8 cells) and the one-row GIZZI CODE wordmark. */
export function helpBannerLines(): string[] {
  // Same layout as the startup header: the wordmark sits beside the mascot, on its middle row.
  const lines = mascotRows("mid").map(ansiArtRow)
  const word = wordmarkRows().map(ansiArtRow)
  const at = Math.floor((lines.length - word.length) / 2)
  word.forEach((w, i) => {
    lines[at + i] = `${lines[at + i]}   ${w}`
  })
  return lines
}
