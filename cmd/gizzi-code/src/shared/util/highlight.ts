/**
 * One highlighter for tool output and summaries, shared by the Ink TUI and
 * the `gizzi run` printer, so a passing test, a failure, a file path or a
 * `+6 −1` count looks the same everywhere.
 *
 * - ✓ / PASS / (pass) / "3 passed" lines: green
 * - ✕ / FAIL / (fail) / error lines, and "1 failed" counts: red
 * - ⚠ / warning lines: amber
 * - file paths: cyan + bold
 * - `+N −M` change counts: green / red
 *
 * Colors go through the chalk shim, so NO_COLOR / FORCE_COLOR=0 / a pipe give
 * plain text, and 256-color terminals (Apple Terminal, tmux) get the nearest
 * hue-keeping palette entry. Lines that already carry ANSI colors (a command
 * that colors its own output) are left alone.
 */
import chalk from "@/shared/util/chalk"

export namespace Highlight {
  // xterm-256 cube colors (114, 167, 178, 74), so truecolor and 256-color
  // terminals show the same thing. Red is a true red, not the hot pink the
  // theme's error color drifts to in 256 colors.
  const green = (s: string) => chalk.rgb(135, 215, 135)(s)
  const red = (s: string) => chalk.rgb(215, 95, 95)(s)
  const amber = (s: string) => chalk.rgb(215, 175, 0)(s)
  const path = (s: string) => chalk.bold.rgb(95, 175, 215)(s)

  // eslint-disable-next-line no-control-regex
  const HAS_ANSI = /\u001b\[/

  const FAIL_LINE =
    /^\s*(?:[✕✗✘×]|\(fail\)|FAIL\b|FAILED\b|not ok\b|error(?:\[\w+\])?:|Error:|\w*Error:|panicked at|E\s{2,}|[1-9]\d* (?:fail(?:ed|ing|ures?)?|errors?)\b)/i
  const FAIL_COUNT = /(?:^|[\s,(])[1-9]\d* (?:failed|failing|failures?|errors?)\b/i
  const PASS_LINE = /^\s*(?:[✓✔]|\(pass\)|PASS\b|ok\b|[1-9]\d* (?:pass(?:ed|ing)?)\b|test result: ok\b)/i
  const WARN_LINE = /^\s*(?:⚠|warn(?:ing)?\b[:\s]|WARN\b)/i

  const EXT =
    "ts|tsx|js|jsx|mjs|cjs|mts|cts|json|jsonc|md|mdx|py|rs|go|rb|java|kt|swift|c|h|cc|cpp|hpp|cs|php|sh|zsh|bash|toml|yaml|yml|css|scss|html|vue|svelte|sql|lock|txt|env|xml|gradle|dart|ex|exs|lua|zig"
  // A path with a slash (src/a.ts, ./x, /abs/y.rs) or a bare file name with a
  // known extension, optionally followed by :line[:col]. Not part of a URL.
  const PATH = new RegExp(
    String.raw`(?<![\w/:.@-])((?:\.{1,2}/|~/|/)?(?:[\w.@-]+/)+[\w.@-]+|[\w@-][\w.@-]*\.(?:${EXT}))(:\d+(?::\d+)?)?(?![\w/])`,
    "g",
  )
  const COUNTS = /(?<![\w+])\+(\d+)(\s+)([−-])(\d+)(?!\w)/g

  /** `+48 −12` in green / red. */
  export function counts(added: number, removed: number): string {
    return `${green(`+${added}`)} ${red(`−${removed}`)}`
  }

  /** Paths and counts inside a line that has no status color of its own. */
  export function inline(line: string): string {
    if (chalk.level === 0 || HAS_ANSI.test(line)) return line
    return line
      .replace(COUNTS, (_m, a: string, gap: string, sign: string, r: string) => `${green(`+${a}`)}${gap}${red(`${sign}${r}`)}`)
      .replace(PATH, (m: string, file: string, pos: string | undefined, offset: number, whole: string) => {
        // Skip URL parts and plain fractions like 3/4.
        if (/^\d+(?:\/\d+)+$/.test(file)) return m
        if (whole.slice(Math.max(0, offset - 3), offset).includes("://")) return m
        return path(file) + (pos ?? "")
      })
  }

  /** Highlight one line of tool output or summary text. */
  export function line(text: string): string {
    if (chalk.level === 0 || !text.trim() || HAS_ANSI.test(text)) return text
    if (FAIL_LINE.test(text) || (FAIL_COUNT.test(text) && !PASS_LINE.test(text))) return red(text)
    if (PASS_LINE.test(text)) return green(text)
    if (WARN_LINE.test(text)) return amber(text)
    return inline(text)
  }

  /** Highlight every line of a block. */
  export function text(block: string): string {
    if (chalk.level === 0) return block
    return block.split("\n").map(line).join("\n")
  }
}
