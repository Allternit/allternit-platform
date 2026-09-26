/**
 * Startup-screen art: the Architectural Sentinel mascot and the GIZZI
 * block wordmark, as data, so WelcomeBox can animate them (beacon pulse,
 * eye blink, shimmer sweep) without hardcoding frames inline.
 *
 * Colors are theme keys, matching the orb spinner: ink body (the theme's
 * `text`, so it reads on light and dark terminals), coral `gizzi` accents,
 * `inactive` legs. The beacon color is passed in because it animates.
 */

export const INK = 'text'
export const CORAL = 'gizzi'
export const LEGS = 'inactive'

export type ArtSegment = [text: string, color: string]
export type ArtRow = ArtSegment[]

/**
 * Sentinel rows with animatable slots. `beacon` (row 0) pulses between
 * coral and its shimmer; the eyes in row 3 swap to '─' during a blink.
 */
export function sentinelRows({
  beaconColor,
  blinking,
}: {
  beaconColor: string
  blinking: boolean
}): ArtRow[] {
  const eyes = blinking ? '─    ─' : '●    ●'
  return [
    [['      ▄▄       ', beaconColor]],
    [['   ▄▄▄  ▄▄▄    ', INK]],
    [[' ▄██████████▄  ', INK]],
    [[' █  ', INK], [eyes, INK], ['  █ ', INK]],
    [[' █  ', INK], ['A : / /', CORAL], [' █ ', INK]],
    [['  ▀████████▀   ', INK]],
    [['   █ █  █ █    ', LEGS]],
    [['   ▀ ▀  ▀ ▀    ', LEGS]],
  ]
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
