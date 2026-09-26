/**
 * Startup-screen art: the Architectural Sentinel mascot and the GIZZI
 * block wordmark, as data, so WelcomeBox can animate them (beacon pulse,
 * eye blink, shimmer sweep) without hardcoding frames inline.
 *
 * Palette matches the orb/welcome identity: sand body, coral accents,
 * structural brown legs, near-black eyes.
 */

export const SAND = '#D4B08C'
export const CORAL = '#D97757'
export const CORAL_BRIGHT = '#F09878'
export const STRUCTURAL = '#8F6F56'
export const EYE = '#111318'

export type ArtSegment = [text: string, color: string]
export type ArtRow = ArtSegment[]

/**
 * Sentinel rows with animatable slots. `beacon` (row 0) pulses between
 * CORAL and CORAL_BRIGHT; the eyes in row 3 swap to '─' during a blink.
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
    [['   ▄▄▄  ▄▄▄    ', SAND]],
    [[' ▄██████████▄  ', SAND]],
    [[' █  ', SAND], [eyes, EYE], ['  █ ', SAND]],
    [[' █  ', SAND], ['A : / /', CORAL], [' █ ', SAND]],
    [['  ▀████████▀   ', SAND]],
    [['   █ █  █ █    ', STRUCTURAL]],
    [['   ▀ ▀  ▀ ▀    ', STRUCTURAL]],
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
