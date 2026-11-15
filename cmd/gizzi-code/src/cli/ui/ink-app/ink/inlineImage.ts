/**
 * Inline images in the cell renderer (iTerm2 / WezTerm OSC 1337, Kitty /
 * Ghostty graphics protocol).
 *
 * A component reserves the image's cells with placeholder characters from the
 * private use area (one column each): a head char registered to an image,
 * then fillers. The log-update writers call inlineImageCellOutput() for every
 * cell they emit. The head draws the image at the cursor without moving it
 * (saved/restored cursor), and every placeholder advances the cursor one
 * column with CUF instead of printing, so nothing overwrites the image and
 * cursor tracking stays exact in both full-frame and diff writes.
 *
 * Terminals without a protocol (Apple Terminal, tmux without passthrough,
 * basic TERMs) get no placeholders: callers check inlineImageProtocol() and render text instead.
 */
export type InlineImageProtocol = 'iterm' | 'kitty'

const HEAD_BASE = 0xe100
export const INLINE_IMAGE_FILLER = ''
const CUF = '\x1b[C'
const SAVE = '\x1b7'
const RESTORE = '\x1b8'

/**
 * Conservative detection from the environment. `force` (the header's
 * `image` mode) skips the tmux refusal and assumes OSC 1337 when nothing
 * is detected.
 */
export function detectInlineImageProtocol(
  env: NodeJS.ProcessEnv = process.env,
  force = false,
  tmuxPassthrough: () => boolean = tmuxAllowsPassthrough,
): InlineImageProtocol | null {
  const override = env.GIZZI_INLINE_IMAGES
  if (override === '0' || override === 'false') return null
  if (override === 'iterm' || override === 'kitty') return override
  if (env.TMUX) {
    // tmux eats image escapes unless `allow-passthrough` is on, and Kitty
    // placements don't survive it; OSC 1337 wrapped in a passthrough DCS does
    // (iTerm2, WezTerm).
    const outerIterm = env.LC_TERMINAL === 'iTerm2' || !!env.WEZTERM_EXECUTABLE
    return (outerIterm || force) && tmuxPassthrough() ? 'iterm' : null
  }
  const program = env.TERM_PROGRAM ?? ''
  const term = env.TERM ?? ''
  if (program === 'iTerm.app' || program === 'WezTerm' || env.WEZTERM_EXECUTABLE) return 'iterm'
  if (program === 'ghostty' || env.GHOSTTY_RESOURCES_DIR || term === 'xterm-ghostty' || term === 'xterm-kitty' || env.KITTY_WINDOW_ID) return 'kitty'
  return force ? 'iterm' : null
}

let passthrough: boolean | undefined
function tmuxAllowsPassthrough(): boolean {
  if (passthrough === undefined) {
    try {
      // eslint-disable-next-line @typescript-eslint/no-require-imports
      const { execFileSync } = require('child_process') as typeof import('child_process')
      const v = execFileSync('tmux', ['show', '-gv', 'allow-passthrough'], { encoding: 'utf8', timeout: 500, stdio: ['ignore', 'pipe', 'ignore'] }).trim()
      passthrough = v === 'on' || v === 'all'
    } catch {
      passthrough = false
    }
  }
  return passthrough
}

let protocol: InlineImageProtocol | null | undefined
let forced: InlineImageProtocol | null | undefined
/** The terminal's image protocol; `force` is the header's `image` mode. */
export function inlineImageProtocol(force = false): InlineImageProtocol | null {
  if (protocol === undefined) protocol = detectInlineImageProtocol()
  if (protocol || !force) return protocol
  if (forced === undefined) forced = detectInlineImageProtocol(process.env, true)
  return forced
}

/** Test hook. */
export function setInlineImageProtocolForTest(p: InlineImageProtocol | null | undefined): void {
  protocol = p
  forced = p === undefined ? undefined : p
}

const heads = new Map<string, string>()
const byKey = new Map<string, string>()

// Kitty image ids: a high base so gizzi's images don't collide with other
// programs' in the same terminal. One placement per image (p=1), so drawing
// the head again moves the image instead of leaving a copy behind.
const KITTY_ID_BASE = 0x47a000
function kittyId(head: string): number {
  return KITTY_ID_BASE + head.charCodeAt(0) - HEAD_BASE
}

function escapeFor(p: InlineImageProtocol, pngBase64: string, cols: number, rows: number, id: number): string {
  if (p === 'iterm') {
    const size = Math.floor((pngBase64.length * 3) / 4)
    return `\x1b]1337;File=inline=1;size=${size};width=${cols};height=${rows};preserveAspectRatio=1;doNotMoveCursor=1:${pngBase64}\x07`
  }
  // Kitty: PNG (f=100), transmit+display (a=T) as image i / placement p=1,
  // c x r cells, C=1 keeps the cursor still, q=2 silences replies. Payload
  // chunked at 4096 bytes.
  let out = ''
  for (let i = 0; i < pngBase64.length; i += 4096) {
    const chunk = pngBase64.slice(i, i + 4096)
    const more = i + 4096 < pngBase64.length ? 1 : 0
    out += i === 0
      ? `\x1b_Gf=100,a=T,i=${id},p=1,c=${cols},r=${rows},C=1,q=2,m=${more};${chunk}\x1b\\`
      : `\x1b_Gm=${more};${chunk}\x1b\\`
  }
  return out
}

/** DCS passthrough: tmux forwards the body to the outer terminal. */
function tmuxWrap(seq: string): string {
  return `\x1bPtmux;${seq.replace(/\x1b/g, '\x1b\x1b')}\x1b\\`
}

/**
 * Placeholder text reserving `cols` cells for an image, or null when the
 * terminal can't show images. `key` dedupes registrations.
 */
export function inlineImagePlaceholder(key: string, pngBase64: string, cols: number, rows = 1, force = false): string | null {
  const p = inlineImageProtocol(force)
  if (!p || cols < 1) return null
  let head = byKey.get(key)
  if (!head) {
    head = String.fromCharCode(HEAD_BASE + byKey.size)
    byKey.set(key, head)
    const seq = escapeFor(p, pngBase64, cols, rows, kittyId(head))
    heads.set(head, SAVE + (process.env.TMUX ? tmuxWrap(seq) : seq) + RESTORE + CUF)
  }
  return head + INLINE_IMAGE_FILLER.repeat(cols - 1)
}

/**
 * The sequence that takes a registered image off the screen, for an image in
 * a live region that can disappear (the pet). Kitty keeps a placement until
 * it's deleted; iTerm2 images are cell content, overwritten like text, so
 * there's nothing to send. Empty when nothing is needed.
 */
export function inlineImageClear(key: string): string {
  const head = byKey.get(key)
  if (!head || inlineImageProtocol() !== 'kitty') return ''
  return `\x1b_Ga=d,d=I,i=${kittyId(head)},q=2\x1b\\`
}

/**
 * Placeholder rows for a multi-row image: the first row starts with the head
 * (the terminal draws the whole `cols` x `rows` image from there); every other
 * cell is a filler so later rows skip the image's cells without printing.
 * Null when the terminal can't show images.
 */
export function inlineImageBlock(key: string, pngBase64: string, cols: number, rows: number, force = false): string[] | null {
  const first = inlineImagePlaceholder(key, pngBase64, cols, rows, force)
  if (first === null) return null
  return [first, ...Array.from({ length: rows - 1 }, () => INLINE_IMAGE_FILLER.repeat(cols))]
}

/** What the writer emits for a cell char: the image/cursor sequence, or the char itself. */
export function inlineImageCellOutput(char: string): string {
  if (char === INLINE_IMAGE_FILLER) return CUF
  if (heads.size > 0) {
    const out = heads.get(char)
    if (out !== undefined) return out
  }
  return char
}
