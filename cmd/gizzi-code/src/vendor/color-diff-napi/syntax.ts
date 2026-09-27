/**
 * Syntax highlighting + line layout for the vendored color-diff TS port.
 *
 * The original color-diff-napi was a Rust NAPI binding whose render() emitted
 * ANSI-highlighted, width-wrapped diff/code lines with a line-number gutter.
 * This module reproduces that behaviour in pure TypeScript so the fast
 * RawAnsi paths in StructuredDiff / HighlightedCode work in compiled builds.
 */

import { rgbSgr } from '@/shared/util/chalk'

export type TokenType =
  | 'keyword'
  | 'string'
  | 'comment'
  | 'number'
  | 'function'
  | 'type'
  | 'identifier'
  | 'property'
  | 'operator'
  | 'punct'
  | 'plain'

export type Span = {
  text: string
  type: TokenType
}

// ---------------------------------------------------------------------------
// Language detection
// ---------------------------------------------------------------------------

const EXT_TO_LANGUAGE: Record<string, string> = {
  ts: 'typescript', tsx: 'typescript', mts: 'typescript', cts: 'typescript',
  js: 'javascript', jsx: 'javascript', mjs: 'javascript', cjs: 'javascript',
  py: 'python', pyi: 'python',
  go: 'go', rs: 'rust', java: 'java', kt: 'java', scala: 'java',
  c: 'c', cc: 'c', cpp: 'c', cxx: 'c', h: 'c', hh: 'c', hpp: 'c', cs: 'c',
  rb: 'ruby', php: 'php', swift: 'c',
  json: 'json', jsonc: 'json', jsonl: 'json',
  sh: 'shell', bash: 'shell', zsh: 'shell', fish: 'shell',
  css: 'css', scss: 'css', less: 'css',
  html: 'html', htm: 'html', xml: 'html', svg: 'html', vue: 'html',
  md: 'markdown', markdown: 'markdown', mdx: 'markdown',
  yml: 'config', yaml: 'config', toml: 'config', ini: 'config', cfg: 'config',
  sql: 'sql',
}

export function resolveLanguage(filePath: string, firstLine?: string | null): string {
  const ext = filePath.split('.').pop()?.toLowerCase() ?? ''
  const lang = EXT_TO_LANGUAGE[ext]
  if (lang) return lang
  // Shebang fallback (extension-less scripts).
  if (firstLine?.startsWith('#!')) {
    if (/\bpython[\d.]*\b/.test(firstLine)) return 'python'
    if (/\b(?:ba|z|fi)?sh\b/.test(firstLine)) return 'shell'
    if (/\bnode\b/.test(firstLine)) return 'javascript'
  }
  return 'text'
}

// ---------------------------------------------------------------------------
// Tokenization — ordered regex rules per language, sticky-matched per line.
// ---------------------------------------------------------------------------

type Rule = [RegExp, TokenType]

const C_KEYWORDS =
  'if else for while do return break continue switch case default try catch finally throw new delete typeof instanceof in of void yield async await static get set class extends super this function const let var import from export default interface type enum implements public private protected readonly abstract'

const PY_KEYWORDS =
  'def class return if elif else for while break continue try except finally raise with as import from pass yield lambda global nonlocal assert del in is not and or None True False self'

const JAVA_KEYWORDS = `${C_KEYWORDS} final package synchronized throws native transient volatile record sealed permits`

const GO_KEYWORDS = `${C_KEYWORDS} package func chan defer go map range struct select fallthrough type switch case default interface`

const RS_KEYWORDS = `${C_KEYWORDS} fn let mut impl trait where match pub mod use crate ref move loop struct enum const static`

const SQL_KEYWORDS =
  'select from where insert into values update set delete create table alter drop index view join left right inner outer full on group by order having limit offset union all distinct as and or not null primary key foreign references unique check default begin commit rollback transaction grant revoke use database between like in is exists case when then else end asc desc'

const SHELL_KEYWORDS =
  'if then elif else fi for in while until do done case esac function select time coproc echo printf read cd pwd export local declare typeset eval exec source return exit set unset shift trap test'

const RE_IDENT = String.raw`[A-Za-z_$][\w$]*`
const RE_C_STRING = String.raw`"(?:[^"\\\n]|\\.)*"?|'(?:[^'\\\n]|\\.)*'?|` + '`(?:[^`\\\\]|\\\\.)*`?'
const RE_PY_STRING = String.raw`(?:[rbfu]{0,2})(?:"""[\s\S]*?"""|'''[\s\S]*?'''|"(?:[^"\\\n]|\\.)*"?|'(?:[^'\\\n]|\\.)*'?)`
const RE_NUMBER = String.raw`\b0[xXbB][0-9a-fA-F_]+\b|\b\d[\d_]*(?:\.\d+)?(?:[eE][+-]?\d+)?[n]?\b`
const RE_C_COMMENT = String.raw`//.*|/\*[\s\S]*?(?:\*/|$)`

function kw(words: string): RegExp {
  return new RegExp(String.raw`\b(?:${words.replace(/ /g, '|')})\b`)
}

const RULES: Record<string, Rule[]> = {
  typescript: [
    [/\/\*\*[\s\S]*?(?:\*\/|$)/, 'comment'],
    [new RegExp(RE_C_COMMENT), 'comment'],
    [new RegExp(RE_C_STRING), 'string'],
    [new RegExp(RE_NUMBER), 'number'],
    [kw(C_KEYWORDS), 'keyword'],
    [new RegExp(String.raw`\b${RE_IDENT}(?=\s*<)`), 'type'],
    [new RegExp(String.raw`\b${RE_IDENT}(?=\s*\()`), 'function'],
    [new RegExp(String.raw`\b[A-Z][A-Za-z0-9_]*\b`), 'type'],
    [new RegExp(String.raw`(?<=\.)${RE_IDENT}\b`), 'property'],
    [new RegExp(String.raw`\b${RE_IDENT}\b`), 'identifier'],
    [/[+\-*/%=<>!&|^~?:]+/, 'operator'],
    [/[{}[\]();,.]/, 'punct'],
  ],
  javascript: [
    [/\/\*\*[\s\S]*?(?:\*\/|$)/, 'comment'],
    [new RegExp(RE_C_COMMENT), 'comment'],
    [new RegExp(RE_C_STRING), 'string'],
    [new RegExp(RE_NUMBER), 'number'],
    [kw(C_KEYWORDS), 'keyword'],
    [new RegExp(String.raw`\b${RE_IDENT}(?=\s*\()`), 'function'],
    [new RegExp(String.raw`\b[A-Z][A-Za-z0-9_]*\b`), 'type'],
    [new RegExp(String.raw`(?<=\.)${RE_IDENT}\b`), 'property'],
    [new RegExp(String.raw`\b${RE_IDENT}\b`), 'identifier'],
    [/[+\-*/%=<>!&|^~?:]+/, 'operator'],
    [/[{}[\]();,.]/, 'punct'],
  ],
  python: [
    [new RegExp(RE_PY_STRING), 'string'],
    [/#.*$/, 'comment'],
    [new RegExp(RE_NUMBER), 'number'],
    [kw(PY_KEYWORDS), 'keyword'],
    [/@\w+/, 'function'],
    [new RegExp(String.raw`\b${RE_IDENT}(?=\s*\()`), 'function'],
    [new RegExp(String.raw`\b[A-Z][A-Za-z0-9_]*\b`), 'type'],
    [new RegExp(String.raw`(?<=\.)${RE_IDENT}\b`), 'property'],
    [new RegExp(String.raw`\b${RE_IDENT}\b`), 'identifier'],
    [/[+\-*/%=<>!&|^~]+/, 'operator'],
    [/[{}[\]();,.:]/, 'punct'],
  ],
  shell: [
    [new RegExp(RE_C_STRING), 'string'],
    [/#.*$/, 'comment'],
    [new RegExp(RE_NUMBER), 'number'],
    [kw(SHELL_KEYWORDS), 'keyword'],
    [new RegExp(String.raw`\$\{[^}]*\}|\$\w+|\$\$|\$[!#?@-]`), 'identifier'],
    [new RegExp(String.raw`\b${RE_IDENT}\b`), 'function'],
    [/[|&;<>()-]+/, 'operator'],
    [/[{}[\]]/, 'punct'],
  ],
  json: [
    [new RegExp(RE_C_STRING), 'string'],
    [new RegExp(RE_NUMBER), 'number'],
    [/\b(?:true|false|null)\b/, 'keyword'],
    [/[{}[\]:,]/, 'punct'],
  ],
  css: [
    [/\/\*[\s\S]*?(?:\*\/|$)/, 'comment'],
    [new RegExp(RE_C_STRING), 'string'],
    [/#[0-9a-fA-F]{3,8}\b/, 'number'],
    [/@[\w-]+/, 'keyword'],
    [/@[\w-]+|[a-zA-Z-]+(?=\s*:[^:])/, 'property'],
    [/[-+]?\d+(?:\.\d+)?(?:px|em|rem|%|vh|vw|s|ms|deg|fr|ch|ex|pt|pc|in|cm|mm)?\b/, 'number'],
    [new RegExp(RE_IDENT), 'identifier'],
    [/[{}:;(),.>+~*=[\]]/, 'punct'],
  ],
  html: [
    [/<!--[\s\S]*?(?:-->|$)/, 'comment'],
    [new RegExp(RE_C_STRING), 'string'],
    [/<\/?[A-Za-z][\w-]*/, 'keyword'],
    [/[\w-]+(?==)/, 'property'],
    [new RegExp(RE_NUMBER), 'number'],
    [/[<>/=]/, 'punct'],
  ],
  markdown: [
    [/^```.*$/, 'keyword'],
    [/^#{1,6}\s.*$/, 'keyword'],
    [/^>.*$/, 'comment'],
    [/`[^`\n]*`/, 'string'],
    [/!\[[^\]]*\]\([^)]*\)/, 'function'],
    [/\[([^\]]*)\]\(([^)]*)\)/, 'function'],
    [/\*\*[^*\n]+\*\*/, 'type'],
    [/^\s*(?:[-*+]|\d+\.)\s/, 'keyword'],
  ],
  config: [
    [/(?:#|;).*$/, 'comment'],
    [new RegExp(RE_C_STRING), 'string'],
    [/^[\w.-]+(?=\s*:)/, 'property'],
    [new RegExp(RE_NUMBER), 'number'],
    [/\b(?:true|false|null|yes|no|on|off)\b/, 'keyword'],
    [/[{}[\],:=-]/, 'punct'],
  ],
  sql: [
    [/--.*$/, 'comment'],
    [/\/\*[\s\S]*?(?:\*\/|$)/, 'comment'],
    [new RegExp(RE_C_STRING), 'string'],
    [new RegExp(RE_NUMBER), 'number'],
    [kw(SQL_KEYWORDS), 'keyword'],
    [new RegExp(String.raw`\b${RE_IDENT}\b`), 'identifier'],
    [/[+\-*/%=<>!]+/, 'operator'],
    [/[(),.;]/, 'punct'],
  ],
  go: [
    [new RegExp(RE_C_COMMENT), 'comment'],
    [new RegExp(RE_C_STRING), 'string'],
    [new RegExp(RE_NUMBER), 'number'],
    [kw(GO_KEYWORDS), 'keyword'],
    [new RegExp(String.raw`\b${RE_IDENT}(?=\s*\()`), 'function'],
    [new RegExp(String.raw`\b[A-Z][A-Za-z0-9_]*\b`), 'type'],
    [new RegExp(String.raw`\b${RE_IDENT}\b`), 'identifier'],
    [/[+\-*/%=<>!&|^~?:]+/, 'operator'],
    [/[{}[\]();,.]/, 'punct'],
  ],
  rust: [
    [new RegExp(RE_C_COMMENT), 'comment'],
    [new RegExp(RE_C_STRING), 'string'],
    [new RegExp(RE_NUMBER), 'number'],
    [kw(RS_KEYWORDS), 'keyword'],
    [new RegExp(String.raw`\b${RE_IDENT}(?=\s*[({])`), 'function'],
    [new RegExp(String.raw`\b[A-Z][A-Za-z0-9_]*\b`), 'type'],
    [new RegExp(String.raw`\b${RE_IDENT}\b`), 'identifier'],
    [/[+\-*/%=<>!&|^~?:@#]+/, 'operator'],
    [/[{}[\]();,.]/, 'punct'],
  ],
  java: [
    [new RegExp(RE_C_COMMENT), 'comment'],
    [new RegExp(RE_C_STRING), 'string'],
    [new RegExp(RE_NUMBER), 'number'],
    [kw(JAVA_KEYWORDS), 'keyword'],
    [new RegExp(String.raw`\b${RE_IDENT}(?=\s*\()`), 'function'],
    [new RegExp(String.raw`\b[A-Z][A-Za-z0-9_]*\b`), 'type'],
    [new RegExp(String.raw`(?<=\.)${RE_IDENT}\b`), 'property'],
    [new RegExp(String.raw`\b${RE_IDENT}\b`), 'identifier'],
    [/[+\-*/%=<>!&|^~?:]+/, 'operator'],
    [/[{}[\]();,.]/, 'punct'],
  ],
  ruby: [
    [new RegExp(RE_PY_STRING), 'string'],
    [/#.*$/, 'comment'],
    [new RegExp(RE_NUMBER), 'number'],
    [kw('def class end if elsif else unless while until do begin rescue ensure return break next yield self nil true false require include module attr_accessor attr_reader attr_writer puts print raise lambda proc'), 'keyword'],
    [/@(?:@)?\w+|[$]\w+/, 'identifier'],
    [new RegExp(String.raw`:${RE_IDENT}\b`), 'string'],
    [new RegExp(String.raw`\b${RE_IDENT}(?=\s*\()`), 'function'],
    [new RegExp(String.raw`\b[A-Z][A-Za-z0-9_]*\b`), 'type'],
    [new RegExp(String.raw`\b${RE_IDENT}\b`), 'identifier'],
    [/[+\-*/%=<>!&|^~]+/, 'operator'],
    [/[{}[\]();,.:]/, 'punct'],
  ],
  php: [],
  text: [
    [new RegExp(RE_C_STRING), 'string'],
    [new RegExp(RE_NUMBER), 'number'],
    [new RegExp(String.raw`\b${RE_IDENT}\b`), 'identifier'],
  ],
}
// php reuses python-ish string/comment handling with c keywords
RULES.php = [
  [new RegExp(RE_C_COMMENT), 'comment'],
  [new RegExp(RE_C_STRING), 'string'],
  [new RegExp(RE_NUMBER), 'number'],
  [kw(`${JAVA_KEYWORDS} echo print fn match`), 'keyword'],
  [new RegExp(String.raw`\$${RE_IDENT}\b`), 'identifier'],
  [new RegExp(String.raw`\b${RE_IDENT}(?=\s*\()`), 'function'],
  [new RegExp(String.raw`\b[A-Z][A-Za-z0-9_]*\b`), 'type'],
  [new RegExp(String.raw`\b${RE_IDENT}\b`), 'identifier'],
  [/[+\-*/%=<>!&|^~?:@]+/, 'operator'],
  [/[{}[\]();,.]/, 'punct'],
]

const combinedCache = new Map<string, RegExp>()

function getCombinedRegex(lang: string): RegExp {
  let re = combinedCache.get(lang)
  if (!re) {
    const rules = RULES[lang] ?? RULES.text!
    re = new RegExp(rules.map((r) => `(${r[0].source})`).join('|'), 'ys')
    combinedCache.set(lang, re)
  }
  return re
}

export function tokenizeLine(line: string, lang: string): Span[] {
  const rules = RULES[lang] ?? RULES.text!
  const re = getCombinedRegex(lang)
  const spans: Span[] = []
  let pos = 0
  while (pos < line.length) {
    re.lastIndex = pos
    const m = re.exec(line)
    if (!m || m.index !== pos) {
      // No rule matched at pos: emit the char as plain and advance.
      const last = spans[spans.length - 1]
      if (last && last.type === 'plain') last.text += line[pos]
      else spans.push({ text: line[pos], type: 'plain' })
      pos++
      continue
    }
    let type: TokenType = 'plain'
    for (let g = 1; g <= rules.length; g++) {
      if (m[g] !== undefined) {
        type = rules[g - 1]![1]
        break
      }
    }
    spans.push({ text: m[0], type })
    pos = re.lastIndex
  }
  return spans
}

// ---------------------------------------------------------------------------
// Color palettes
// ---------------------------------------------------------------------------

export type Rgb = [number, number, number]

export type TokenPalette = Record<TokenType, string | undefined>

export const DARK_TOKENS: TokenPalette = {
  keyword: '#569cd6', string: '#ce9178', comment: '#6a9955', number: '#b5cea8',
  function: '#dcdcaa', type: '#4ec9b0', identifier: '#9cdcfe', property: '#9cdcfe',
  operator: undefined, punct: undefined, plain: '#d4d4d4',
}

export const LIGHT_TOKENS: TokenPalette = {
  keyword: '#0000ff', string: '#a31515', comment: '#008000', number: '#098658',
  function: '#795e26', type: '#267f99', identifier: '#001080', property: '#001080',
  operator: undefined, punct: undefined, plain: '#000000',
}

export type DiffPalette = {
  added: Rgb
  removed: Rgb
  addedDim: Rgb
  removedDim: Rgb
  /** Gutter marker/number colors on tinted rows (theme diff*Word). */
  addedWord: Rgb
  removedWord: Rgb
  /** true → emit SGR palette colors instead of truecolor (dark-ansi/light-ansi themes) */
  ansi: boolean
  tokens: TokenPalette
}

export function mapThemeName(themeName: string): {
  palette: DiffPalette
  syntaxThemeName: string
} {
  const name = (themeName || 'dark').toLowerCase()
  const lightTokens = name.startsWith('light')
  const daltonized = name.includes('daltonized')
  const ansi = name.includes('ansi')
  const palette: DiffPalette = {
    ansi,
    tokens: lightTokens ? LIGHT_TOKENS : DARK_TOKENS,
    // Values mirror src/cli/ui/ink-app/utils/theme.ts diff backgrounds.
    ...(lightTokens
      ? daltonized
        ? { added: [153, 204, 255] as Rgb, removed: [255, 204, 204] as Rgb, addedDim: [209, 231, 253] as Rgb, removedDim: [255, 233, 233] as Rgb, addedWord: [51, 102, 204] as Rgb, removedWord: [153, 51, 51] as Rgb }
        : { added: [105, 219, 124] as Rgb, removed: [255, 168, 180] as Rgb, addedDim: [199, 225, 203] as Rgb, removedDim: [253, 210, 216] as Rgb, addedWord: [47, 157, 68] as Rgb, removedWord: [209, 69, 75] as Rgb }
      : daltonized
        ? { added: [0, 68, 102] as Rgb, removed: [102, 0, 0] as Rgb, addedDim: [62, 81, 91] as Rgb, removedDim: [62, 44, 44] as Rgb, addedWord: [0, 119, 179] as Rgb, removedWord: [179, 0, 0] as Rgb }
        : { added: [34, 92, 43] as Rgb, removed: [122, 41, 54] as Rgb, addedDim: [71, 88, 74] as Rgb, removedDim: [105, 72, 77] as Rgb, addedWord: [56, 166, 96] as Rgb, removedWord: [179, 89, 107] as Rgb }),
  }
  const syntaxThemeName = lightTokens ? 'light-plus' : 'dark-plus'
  return { palette, syntaxThemeName }
}

// ---------------------------------------------------------------------------
// ANSI emission + width wrapping
// ---------------------------------------------------------------------------

export const ANSI_RESET = '\x1b[0m'
const SGR_DIM = '\x1b[2m'

function fgFor(type: TokenType, tokens: TokenPalette, ansi: boolean): string {
  const hex = tokens[type] ?? tokens.plain
  if (!hex) return ''
  if (ansi) return `\x1b[${type === 'comment' ? '32' : '37'}m`
  const int = parseInt(hex.replace('#', ''), 16)
  // Follows the terminal's color level (256 colors in Apple Terminal before
  // macOS 26, truecolor elsewhere) like the rest of the TUI.
  return rgbSgr((int >> 16) & 255, (int >> 8) & 255, int & 255)
}

export function fgAnsi(rgb: Rgb, ansi: boolean, fallback: string): string {
  return ansi ? fallback : rgbSgr(rgb[0], rgb[1], rgb[2])
}

export function bgAnsi(rgb: Rgb, ansi: boolean): string {
  return ansi ? '\x1b[42m' : rgbSgr(rgb[0], rgb[1], rgb[2], true)
}

export type StyledSpan = { text: string; fg?: string; bg?: string; dim?: boolean }

/** Wrap styled spans to a printable width, hard-breaking overlong spans. */
export function wrapSpans(spans: StyledSpan[], width: number): StyledSpan[][] {
  if (width <= 0) return [[]]
  const lines: StyledSpan[][] = []
  let current: StyledSpan[] = []
  let col = 0
  const push = (s: StyledSpan) => {
    current.push(s)
    col += s.text.length
  }
  for (const span of spans) {
    let remaining = span.text
    while (remaining.length > 0) {
      const avail = width - col
      if (avail <= 0) {
        lines.push(current)
        current = []
        col = 0
        continue
      }
      const take = remaining.slice(0, avail)
      push({ ...span, text: take })
      remaining = remaining.slice(avail)
    }
  }
  lines.push(current)
  return lines
}

/** Serialize styled spans to an ANSI string. */
export function emitSpans(spans: StyledSpan[], opts: { dimAll?: boolean } = {}): string {
  let out = ''
  for (const s of spans) {
    if (s.text.length === 0) continue
    if (s.bg) out += s.bg
    if (s.fg) out += s.fg
    if (s.dim || opts.dimAll) out += SGR_DIM
    out += s.text
    if (s.bg || s.fg || s.dim || opts.dimAll) out += ANSI_RESET
  }
  return out
}

/** Highlight one line of code into styled spans. */
export function highlightSpans(
  code: string,
  lang: string,
  palette: DiffPalette,
  bg?: string,
  dim = false,
): StyledSpan[] {
  const spans = tokenizeLine(code, lang)
  return spans.map((s) => ({
    text: s.text,
    fg: fgFor(s.type, palette.tokens, palette.ansi),
    bg,
    dim,
  }))
}

export { SGR_DIM }
