/**
 * Minimal chalk-compatible styling shim.
 *
 * chalk was a phantom dependency: imported across the app but never declared
 * in package.json (it resolved via the workspace root's node_modules). This
 * shim covers exactly the API surface the app uses — the 16/256/truecolor
 * styles, rgb/hex/ansi256 builders with down-sampling, chaining, and a
 * mutable `level` — so the codebase no longer depends on the undeclared
 * package. Terminal color support detection mirrors chalk: FORCE_COLOR >
 * NO_COLOR > TERM=dumb > isTTY.
 */

export interface ChalkShim {
  (text: unknown): string
  /** Color support level: 0 = none, 1 = 16 colors, 2 = 256, 3 = truecolor. */
  level: number
  readonly reset: ChalkShim
  readonly bold: ChalkShim
  readonly dim: ChalkShim
  readonly italic: ChalkShim
  readonly underline: ChalkShim
  readonly inverse: ChalkShim
  readonly strikethrough: ChalkShim
  readonly black: ChalkShim
  readonly red: ChalkShim
  readonly green: ChalkShim
  readonly yellow: ChalkShim
  readonly blue: ChalkShim
  readonly magenta: ChalkShim
  readonly cyan: ChalkShim
  readonly white: ChalkShim
  readonly gray: ChalkShim
  readonly grey: ChalkShim
  readonly blackBright: ChalkShim
  readonly redBright: ChalkShim
  readonly greenBright: ChalkShim
  readonly yellowBright: ChalkShim
  readonly blueBright: ChalkShim
  readonly magentaBright: ChalkShim
  readonly cyanBright: ChalkShim
  readonly whiteBright: ChalkShim
  readonly bgBlack: ChalkShim
  readonly bgRed: ChalkShim
  readonly bgGreen: ChalkShim
  readonly bgYellow: ChalkShim
  readonly bgBlue: ChalkShim
  readonly bgMagenta: ChalkShim
  readonly bgCyan: ChalkShim
  readonly bgWhite: ChalkShim
  readonly bgGray: ChalkShim
  readonly bgGrey: ChalkShim
  readonly bgBlackBright: ChalkShim
  readonly bgRedBright: ChalkShim
  readonly bgGreenBright: ChalkShim
  readonly bgYellowBright: ChalkShim
  readonly bgBlueBright: ChalkShim
  readonly bgMagentaBright: ChalkShim
  readonly bgCyanBright: ChalkShim
  readonly bgWhiteBright: ChalkShim
  rgb(red: number, green: number, blue: number): ChalkShim
  bgRgb(red: number, green: number, blue: number): ChalkShim
  hex(color: string): ChalkShim
  bgHex(color: string): ChalkShim
  ansi256(code: number): ChalkShim
  bgAnsi256(code: number): ChalkShim
}

type Style = { opens: string[]; closes: string[] }

const EMPTY: Style = { opens: [], closes: [] }

const STYLE_CODES: Record<string, string> = {
  reset: '0',
  bold: '1',
  dim: '2',
  italic: '3',
  underline: '4',
  inverse: '7',
  strikethrough: '9',
  black: '30',
  red: '31',
  green: '32',
  yellow: '33',
  blue: '34',
  magenta: '35',
  cyan: '36',
  white: '37',
  gray: '90',
  grey: '90',
  blackBright: '90',
  redBright: '91',
  greenBright: '92',
  yellowBright: '93',
  blueBright: '94',
  magentaBright: '95',
  cyanBright: '96',
  whiteBright: '97',
  bgBlack: '40',
  bgRed: '41',
  bgGreen: '42',
  bgYellow: '43',
  bgBlue: '44',
  bgMagenta: '45',
  bgCyan: '46',
  bgWhite: '47',
  bgGray: '100',
  bgGrey: '100',
  bgBlackBright: '100',
  bgRedBright: '101',
  bgGreenBright: '102',
  bgYellowBright: '103',
  bgBlueBright: '104',
  bgMagentaBright: '105',
  bgCyanBright: '106',
  bgWhiteBright: '107',
}

function closeCode(open: string): string {
  if (open.startsWith('38')) return '39'
  if (open.startsWith('48')) return '49'
  const numeric = Number(open)
  if (numeric >= 30 && numeric <= 37) return '39'
  if (numeric >= 90 && numeric <= 97) return '39'
  if (numeric >= 40 && numeric <= 47) return '49'
  if (numeric >= 100 && numeric <= 107) return '49'
  switch (open) {
    case '1':
    case '2':
      return '22'
    case '3':
      return '23'
    case '4':
      return '24'
    case '7':
      return '27'
    case '9':
      return '29'
    default:
      return '0'
  }
}

function withOpen(style: Style, open: string): Style {
  return { opens: [...style.opens, open], closes: [closeCode(open), ...style.closes] }
}

// Standard ANSI 16-color palette (normal + bright), used for down-sampling.
const ANSI16_RGB: Array<[number, number, number]> = [
  [0, 0, 0], [128, 0, 0], [0, 128, 0], [128, 128, 0],
  [0, 0, 128], [128, 0, 128], [0, 128, 128], [192, 192, 192],
  [128, 128, 128], [255, 0, 0], [0, 255, 0], [255, 255, 0],
  [0, 0, 255], [255, 0, 255], [0, 255, 255], [255, 255, 255],
]

function nearestAnsi16(red: number, green: number, blue: number): number {
  let best = 0
  let bestDistance = Infinity
  for (let i = 0; i < ANSI16_RGB.length; i++) {
    const [r, g, b] = ANSI16_RGB[i]
    const distance = (r - red) ** 2 + (g - green) ** 2 + (b - blue) ** 2
    if (distance < bestDistance) {
      bestDistance = distance
      best = i
    }
  }
  return best
}

function toAnsi256(red: number, green: number, blue: number): string {
  const channel = (value: number) => Math.round((value / 255) * 5)
  return `5;${16 + 36 * channel(red) + 6 * channel(green) + channel(blue)}`
}

function fg(red: number, green: number, blue: number): string {
  if (currentLevel() >= 3) return `38;2;${red};${green};${blue}`
  if (currentLevel() === 2) return `38;${toAnsi256(red, green, blue)}`
  const nearest = nearestAnsi16(red, green, blue)
  return nearest < 8 ? `3${nearest}` : `9${nearest - 8}`
}

function bg(red: number, green: number, blue: number): string {
  if (currentLevel() >= 3) return `48;2;${red};${green};${blue}`
  if (currentLevel() === 2) return `48;${toAnsi256(red, green, blue)}`
  const nearest = nearestAnsi16(red, green, blue)
  return nearest < 8 ? `4${nearest}` : `10${nearest - 8}`
}

function parseHex(color: string): [number, number, number] {
  let value = color.replace(/^#/, '')
  if (value.length === 3) {
    value = value.split('').map((c) => c + c).join('')
  }
  if (!/^[0-9a-fA-F]{6}$/.test(value)) return [255, 255, 255]
  return [
    parseInt(value.slice(0, 2), 16),
    parseInt(value.slice(2, 4), 16),
    parseInt(value.slice(4, 6), 16),
  ]
}

function detectLevel(): number {
  const force = process.env.FORCE_COLOR
  if (force !== undefined) {
    if (force === 'false' || force === '0') return 0
    if (force === 'truecolor' || force === '3') return 3
    if (force === '2') return 2
    return 1
  }
  if (process.env.NO_COLOR !== undefined) return 0
  if (process.env.TERM === 'dumb') return 0
  return process.stdout?.isTTY ? 3 : 0
}

let level = detectLevel()

function currentLevel(): number {
  return level
}

function apply(style: Style, text: unknown): string {
  const str = String(text)
  if (level === 0 || style.opens.length === 0) return str
  return `\x1b[${style.opens.join(';')}m${str}\x1b[${style.closes.join(';')}m`
}

function makeShim(style: Style): ChalkShim {
  const shim = ((text: unknown) => apply(style, text)) as ChalkShim

  Object.defineProperty(shim, 'level', {
    get: () => level,
    set: (value: number) => {
      level = Math.max(0, Math.min(3, Math.trunc(value)))
    },
    enumerable: true,
  })

  const derived = new Map<string, ChalkShim>()
  const styleProp = (name: string): ChalkShim => {
    let cached = derived.get(name)
    if (!cached) {
      cached = makeShim(withOpen(style, STYLE_CODES[name]))
      derived.set(name, cached)
    }
    return cached
  }

  for (const name of Object.keys(STYLE_CODES)) {
    Object.defineProperty(shim, name, { get: () => styleProp(name), enumerable: true })
  }

  shim.rgb = (red, green, blue) => makeShim(withOpen(style, fg(red, green, blue)))
  shim.bgRgb = (red, green, blue) => makeShim(withOpen(style, bg(red, green, blue)))
  shim.hex = (color) => {
    const [r, g, b] = parseHex(color)
    return makeShim(withOpen(style, fg(r, g, b)))
  }
  shim.bgHex = (color) => {
    const [r, g, b] = parseHex(color)
    return makeShim(withOpen(style, bg(r, g, b)))
  }
  shim.ansi256 = (code) => {
    const open = level >= 2 ? `38;5;${code}` : level === 1 ? fg(code, code, code) : ''
    return makeShim(open ? withOpen(style, open) : style)
  }
  shim.bgAnsi256 = (code) => {
    const open = level >= 2 ? `48;5;${code}` : level === 1 ? bg(code, code, code) : ''
    return makeShim(open ? withOpen(style, open) : style)
  }

  return shim
}

const chalk: ChalkShim = makeShim(EMPTY)

export default chalk
