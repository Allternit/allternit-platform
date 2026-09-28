import { renderPixelArt, type ArtRow } from '../components/welcomeArt'
import { gizziPetRows, GIZZI_PET_WIDTH, type GizziPose } from './gizziSprite'
import type { PetAvatar, PetBot } from './petBots'

/**
 * The pet sprite for any bot, in the same 12x10 pixel slot (12 columns x 5
 * rows of half blocks) as the Gizzi pet. Gizzi keeps its official mark;
 * geometric and pet avatars are drawn from their stored shape/species,
 * colors, eyes and accessory (allternit-ai BotAvatar), and animate with the
 * same poses: glance lifts the eyes, blink and wink close them.
 *
 * Pixel keys: P primary, S secondary (face panel), E eye, N nose/antenna,
 * K dark detail, B bow, '.' transparent.
 */
export const PET_SPRITE_WIDTH = GIZZI_PET_WIDTH

const SHAPE_MASKS: Record<Extract<PetAvatar, { kind: 'geometric' }>['shape'], string[]> = {
  circle: [
    '....PPPP....',
    '..PPPPPPPP..',
    '.PPPPPPPPPP.',
    '.PSSSSSSSSP.',
    'PPSSSSSSSSPP',
    'PPSSSSSSSSPP',
    '.PSSSSSSSSP.',
    '.PPPPPPPPPP.',
    '..PPPPPPPP..',
    '....PPPP....',
  ],
  rounded: [
    '............',
    '.PPPPPPPPPP.',
    'PPPPPPPPPPPP',
    'PPSSSSSSSSPP',
    'PPSSSSSSSSPP',
    'PPSSSSSSSSPP',
    'PPSSSSSSSSPP',
    'PPPPPPPPPPPP',
    '.PPPPPPPPPP.',
    '............',
  ],
  square: [
    '............',
    'PPPPPPPPPPPP',
    'PPPPPPPPPPPP',
    'PPSSSSSSSSPP',
    'PPSSSSSSSSPP',
    'PPSSSSSSSSPP',
    'PPSSSSSSSSPP',
    'PPPPPPPPPPPP',
    'PPPPPPPPPPPP',
    '............',
  ],
  hex: [
    '...PPPPPP...',
    '..PPPPPPPP..',
    '.PPPPPPPPPP.',
    'PPSSSSSSSSPP',
    'PPSSSSSSSSPP',
    'PPSSSSSSSSPP',
    'PPSSSSSSSSPP',
    '.PPPPPPPPPP.',
    '..PPPPPPPP..',
    '...PPPPPP...',
  ],
  diamond: [
    '.....PP.....',
    '....PPPP....',
    '...PPPPPP...',
    '..PSSSSSSP..',
    '.PSSSSSSSSP.',
    '.PSSSSSSSSP.',
    '..PSSSSSSP..',
    '...PPPPPP...',
    '....PPPP....',
    '.....PP.....',
  ],
}

// [col, row] eye pixels per preset, on the face panel (rows 4-5).
const EYE_PIXELS: Record<Extract<PetAvatar, { kind: 'geometric' }>['eyes'], Array<[number, number]>> = {
  round: [[3, 4], [4, 4], [3, 5], [4, 5], [7, 4], [8, 4], [7, 5], [8, 5]],
  wide: [[2, 4], [3, 4], [4, 4], [7, 4], [8, 4], [9, 4]],
  narrow: [[3, 5], [4, 5], [7, 5], [8, 5]],
  focused: [[4, 4], [4, 5], [7, 4], [7, 5]],
  curious: [[3, 4], [4, 4], [3, 5], [4, 5], [8, 4]],
}

type SpeciesTemplate = { pixels: string[]; face: 'P' | 'S' }
const SPECIES: Record<Extract<PetAvatar, { kind: 'pet' }>['species'], SpeciesTemplate> = {
  cat: {
    face: 'P',
    pixels: [
      '.P........P.',
      '.PP......PP.',
      '.PPPPPPPPPP.',
      'PPPPPPPPPPPP',
      'PPEEPPPPEEPP',
      'PPEEPPPPEEPP',
      'PPPSSNNSSPPP',
      '.PPSSSSSSPP.',
      '..PPPPPPPP..',
      '...P....P...',
    ],
  },
  dog: {
    face: 'P',
    pixels: [
      '............',
      '..PPPPPPPP..',
      'SSPPPPPPPPSS',
      'SSPPPPPPPPSS',
      'SSPEEPPEEPSS',
      'SSPEEPPEEPSS',
      'S.PPSNNSPP.S',
      '..PPSSSSPP..',
      '...PPPPPP...',
      '...P....P...',
    ],
  },
  rabbit: {
    face: 'P',
    pixels: [
      '...PS..SP...',
      '...PS..SP...',
      '...PP..PP...',
      '..PPPPPPPP..',
      '.PPEEPPEEPP.',
      '.PPEEPPEEPP.',
      '.PPPSNNSPPP.',
      '..PPSSSSPP..',
      '...PPPPPP...',
      '...P....P...',
    ],
  },
  fox: {
    face: 'P',
    pixels: [
      'P..........P',
      'PP........PP',
      'PPP......PPP',
      'PPPPPPPPPPPP',
      'PPEEPPPPEEPP',
      'SPEEPPPPEEPS',
      'SSSPPNNPPSSS',
      '.SSSSSSSSSS.',
      '...SSSSSS...',
      '....S..S....',
    ],
  },
  owl: {
    face: 'S',
    pixels: [
      '.P........P.',
      '.PPPPPPPPPP.',
      'PPSSSPPSSSPP',
      'PSEESPPSEESP',
      'PSEESPPSEESP',
      'PPSSSNNSSSPP',
      'PPPPPNNPPPPP',
      'PPSPSPPSPSPP',
      '.PPPPPPPPPP.',
      '..N......N..',
    ],
  },
  robot: {
    face: 'S',
    pixels: [
      '.....NN.....',
      '.....PP.....',
      '.PPPPPPPPPP.',
      '.PSSSSSSSSP.',
      'PPSEESSEESPP',
      'PPSEESSEESPP',
      '.PSSSSSSSSP.',
      '.PPPPPPPPPP.',
      '..PP.PP.PP..',
      '..P......P..',
    ],
  },
}

const DARK = '#1D1B1A'
const LIGHT = '#F5F1EB'
const NOSE = '#E8878F'
const BOW = '#E86A92'

function luminance(hex: string): number {
  const n = Number.parseInt(hex.slice(1), 16)
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map(v => {
    const c = v / 255
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!
}

/** Eyes read against the face panel: light eyes on dark faces, dark on light. */
export function eyeColorOn(face: string): string {
  return luminance(face) < 0.35 ? LIGHT : DARK
}

/** Apply a pose to the eye pixels ('E') of a grid; `face` fills closed/moved eyes. */
function pose(grid: string[][], p: GizziPose, face: string): void {
  const eyes: Array<[number, number]> = []
  grid.forEach((row, r) => row.forEach((px, c) => px === 'E' && eyes.push([c, r])))
  if (eyes.length === 0 || p === 'idle') return
  const rows = [...new Set(eyes.map(([, r]) => r))].sort((a, b) => a - b)
  const top = rows[0]!
  const bottom = rows[rows.length - 1]!
  const mid = PET_SPRITE_WIDTH / 2
  for (const [c, r] of eyes) {
    const closing = p === 'blink' || (p === 'wink' && c >= mid)
    if (closing) grid[r]![c] = r === bottom ? 'K' : face
  }
  if (p === 'glance' && top > 0) {
    // Look up: shift the eyes one pixel row higher where the face continues.
    for (const [c, r] of eyes) {
      if (grid[r - 1]?.[c] === face) {
        grid[r - 1]![c] = 'E'
        grid[r]![c] = face
      }
    }
  }
}

function geometricPixels(avatar: Extract<PetAvatar, { kind: 'geometric' }>): string[][] {
  const grid = SHAPE_MASKS[avatar.shape].map(r => r.split(''))
  for (const [c, r] of EYE_PIXELS[avatar.eyes]) if (grid[r]?.[c] === 'S') grid[r]![c] = 'E'
  return grid
}

function speciesPixels(avatar: Extract<PetAvatar, { kind: 'pet' }>): string[][] {
  const grid = SPECIES[avatar.species].pixels.map(r => r.split(''))
  const eyeRow = grid.findIndex(r => r.includes('E'))
  if (avatar.accessory === 'glasses' && eyeRow >= 0) {
    // Frame bridge between the eyes.
    const row = grid[eyeRow]!
    const first = row.indexOf('E')
    const last = row.lastIndexOf('E')
    for (let c = first; c <= last; c++) if (row[c] !== 'E') row[c] = 'K'
  } else if (avatar.accessory === 'bow') {
    const top = grid[0]!
    top[5] = 'B'
    top[6] = 'B'
  } else if (avatar.accessory === 'headset' && eyeRow >= 0) {
    for (const r of [eyeRow - 1, eyeRow, eyeRow + 1]) {
      if (grid[r]) {
        grid[r]![0] = 'K'
        grid[r]![PET_SPRITE_WIDTH - 1] = 'K'
      }
    }
  }
  return grid
}

/** The pet's rows for a bot and pose. `beacon` colors Gizzi's beacon (theme key). */
export function petBotRows(bot: PetBot, p: GizziPose, beacon: string = 'gizzi'): ArtRow[] {
  const avatar = bot.avatar
  // Image avatars draw as images where the terminal can (AvatarImage); this is the fallback.
  if (avatar.kind === 'gizzi' || avatar.kind === 'image') return gizziPetRows(p, beacon)
  const grid = avatar.kind === 'geometric' ? geometricPixels(avatar) : speciesPixels(avatar)
  const faceKey = avatar.kind === 'pet' ? SPECIES[avatar.species].face : 'S'
  const faceColor = faceKey === 'S' ? avatar.secondary : avatar.primary
  pose(grid, p, faceKey)
  const colors: Record<string, string> = {
    P: avatar.primary,
    S: avatar.secondary,
    E: eyeColorOn(faceColor),
    N: avatar.kind === 'pet' && avatar.species === 'robot' ? beacon : NOSE,
    K: eyeColorOn(faceColor) === LIGHT ? LIGHT : DARK,
    B: BOW,
  }
  return renderPixelArt(
    grid.map(r => r.join('')),
    px => colors[px],
  )
}

/** One-line face for narrow terminals: `▐■ ■▌` in the bot's colors. */
export function petFaceColors(bot: PetBot): { body: string; face: string; eye: string } {
  const avatar = bot.avatar
  if (avatar.kind === 'gizzi' || avatar.kind === 'image') return { body: 'gizziSand', face: 'gizziVisor', eye: 'gizziEye' }
  const face = avatar.kind === 'pet' && SPECIES[avatar.species].face === 'P' ? avatar.primary : avatar.secondary
  return { body: avatar.primary, face, eye: eyeColorOn(face) }
}
