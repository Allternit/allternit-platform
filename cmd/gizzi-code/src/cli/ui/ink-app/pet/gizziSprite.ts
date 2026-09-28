import { renderPixelArt, SAND, VISOR, EYE, CORAL, type ArtRow } from '../components/welcomeArt'

/**
 * The pet: a small Gizzi drawn from the official mascot proportions
 * (Allternit Assets/Brand/Gizzi), 12x10 pixels = 12 columns x 5 rows, the
 * same slot the species sprites used. Beacon, ear pods, head, face panel,
 * coral nose, hands, four legs. Poses are the mark's animation states:
 * glance (eyes up toward the beacon) and wink.
 */
export type GizziPose = 'idle' | 'blink' | 'glance' | 'wink'

const BASE = [
  '.....BB.....',
  '..TTT..TTT..',
  '............',
  '.TTTTTTTTTT.',
  '.TVVVVVVVVT.',
  'TTVVVVVVVVTT',
  'TTVVVVVVVVTT',
  '.TVVVCVVVVT.',
  '.TTTTTTTTTT.',
  '..T.T..T.T..',
]

// [col, row] eye pixels per pose. Eyes sit in one cell row (pixel rows 4-5)
// so each eye is a clean full block; glance keeps only the upper half (looking
// up at the beacon), wink closes the right eye to a low line.
const EYES: Record<GizziPose, Array<[number, number]>> = {
  idle: [[3, 4], [3, 5], [8, 4], [8, 5]],
  blink: [],
  glance: [[3, 4], [8, 4]],
  wink: [[3, 4], [3, 5], [7, 5], [8, 5]],
}

export const GIZZI_PET_WIDTH = BASE[0]!.length

export function gizziPetRows(pose: GizziPose, beaconColor: string = CORAL): ArtRow[] {
  const pixels = BASE.map(r => r.split(''))
  for (const [c, r] of EYES[pose]) pixels[r]![c] = 'E'
  const colorOf = (p: string): string | undefined =>
    p === 'B' ? beaconColor : p === 'T' ? SAND : p === 'V' ? VISOR : p === 'E' ? EYE : p === 'C' ? CORAL : undefined
  return renderPixelArt(pixels.map(r => r.join('')), colorOf)
}
