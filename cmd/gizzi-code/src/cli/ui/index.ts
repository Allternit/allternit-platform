import z from "zod/v4"
import { EOL } from "os"
import { NamedError } from "@allternit/gizzi-util/error.js"
import { logo as glyphs } from "@/cli/ui/logo"
import chalk from "@/shared/util/chalk"
import { Highlight } from "@/shared/util/highlight"

export namespace UI {
  export const CancelledError = NamedError.create("UICancelledError", z.void())

  // NO_COLOR (https://no-color.org) turns every style off; FORCE_COLOR wins.
  const plain =
    (process.env.NO_COLOR !== undefined && process.env.FORCE_COLOR === undefined) ||
    process.env.FORCE_COLOR === "0" ||
    process.env.FORCE_COLOR === "false"
  const sgr = (code: string) => (plain ? "" : code)

  export const Style = {
    RESET: sgr("\x1b[0m"),
    TEXT_HIGHLIGHT: sgr("\x1b[96m"),
    TEXT_HIGHLIGHT_BOLD: sgr("\x1b[96m\x1b[1m"),
    TEXT_DIM: sgr("\x1b[90m"),
    TEXT_DIM_BOLD: sgr("\x1b[90m\x1b[1m"),
    TEXT_NORMAL: sgr("\x1b[0m"),
    TEXT_NORMAL_BOLD: sgr("\x1b[1m"),
    TEXT_BOLD: sgr("\x1b[1m"),
    TEXT_WARNING: sgr("\x1b[93m"),
    TEXT_WARNING_BOLD: sgr("\x1b[93m\x1b[1m"),
    TEXT_DANGER: sgr("\x1b[91m"),
    TEXT_DANGER_BOLD: sgr("\x1b[91m\x1b[1m"),
    TEXT_ERROR: sgr("\x1b[91m"),
    TEXT_SUCCESS: sgr("\x1b[92m"),
    TEXT_SUCCESS_BOLD: sgr("\x1b[92m\x1b[1m"),
    TEXT_INFO: sgr("\x1b[94m"),
    TEXT_INFO_BOLD: sgr("\x1b[94m\x1b[1m"),
  }

  export function println(...message: string[]) {
    print(...message)
    Bun.stderr.write(EOL)
  }

  export function print(...message: string[]) {
    blank = false
    Bun.stderr.write(message.join(" "))
  }

  let blank = false
  export function empty() {
    if (blank) return
    println("" + Style.TEXT_NORMAL)
    blank = true
  }

  export function logo(pad?: string) {
    const result: string[] = []
    const reset = "\x1b[0m"
    const left = {
      fg: "\x1b[38;2;212;176;140m", // #D4B08C (Sand)
      shadow: "\x1b[38;2;42;33;26m", // #2A211A (Dark Sand)
      bg: "\x1b[48;2;26;22;18m", // #1A1612 (Obsidian)
    }
    const right = {
      fg: "\x1b[38;2;212;176;140m", // #D4B08C (Sand)
      shadow: "\x1b[38;2;42;33;26m", // #2A211A (Dark Sand)
      bg: "\x1b[48;2;26;22;18m", // #1A1612 (Obsidian)
    }
    const gap = " "
    const draw = (line: string, fg: string, shadow: string, bg: string) => {
      const parts: string[] = []
      for (const char of line) {
        if (char === "_") {
          parts.push(bg, " ", reset)
          continue
        }
        if (char === "^") {
          parts.push(fg, bg, "▀", reset)
          continue
        }
        if (char === "~") {
          parts.push(shadow, "▀", reset)
          continue
        }
        if (char === "#") {
          parts.push(fg, "█", reset)
          continue
        }
        if (char === "*") {
          parts.push(shadow, "█", reset)
          continue
        }
        if (char === " ") {
          parts.push(" ")
          continue
        }
        parts.push(fg, char, reset)
      }
      return parts.join("")
    }
    glyphs.left.forEach((row, index) => {
      if (pad) result.push(pad)
      result.push(draw(row, left.fg, left.shadow, left.bg))
      result.push(gap)
      const other = glyphs.right[index] ?? ""
      result.push(draw(other, right.fg, right.shadow, right.bg))
      result.push(EOL)
    })
    const combined = result.join("").trimEnd()
    const coffee = `\n\n  ${reset}GIZZI ${left.fg}CODE${reset}\n`
    return combined + coffee
  }

  export async function input(prompt: string): Promise<string> {
    const readline = require("readline")
    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stdout,
    })

    return new Promise((resolve) => {
      rl.question(prompt, (answer: string) => {
        rl.close()
        resolve(answer.trim())
      })
    })
  }

  export function error(message: string) {
    if (message.startsWith("Error: ")) {
      message = message.slice("Error: ".length)
    }
    println(Style.TEXT_DANGER_BOLD + "Error: " + Style.TEXT_NORMAL + message)
  }

  export function info(message: string) {
    println(Style.TEXT_NORMAL + message)
  }

  export function success(message: string) {
    println(Style.TEXT_SUCCESS_BOLD + "✓ " + Style.TEXT_NORMAL + message)
  }

  export function warn(message: string) {
    println(Style.TEXT_WARNING_BOLD + "⚠ " + Style.TEXT_NORMAL + message)
  }

  export function markdown(text: string): string {
    const lines = text.split("\n")
    const out: string[] = []
    let inCode = false
    const S = Style
    const code = (s: string) => chalk.rgb(177, 185, 249)(s)
    // Bold, italics and inline code; paths and +N −M counts outside code spans.
    const inline = (line: string) =>
      line
        .split(/(`[^`]+`)/)
        .map((seg) => {
          if (seg.length > 2 && seg.startsWith("`") && seg.endsWith("`")) return code(seg.slice(1, -1))
          return Highlight.inline(seg)
            .replace(/\*\*(.+?)\*\*/g, `${S.TEXT_NORMAL_BOLD}$1${S.TEXT_NORMAL}`)
            .replace(/(^|[\s(])\*([^*\s][^*]*?)\*(?=[\s).,:;!?]|$)/g, `$1${chalk.italic("$2")}`)
        })
        .join("")

    for (const line of lines) {
      if (line.trimStart().startsWith("```")) {
        inCode = !inCode
        if (inCode) {
          const lang = line.trim().slice(3).trim()
          if (lang) out.push(S.TEXT_DIM + "  " + lang + S.TEXT_NORMAL)
        }
        continue
      }
      if (inCode) {
        out.push(S.TEXT_DIM + "  │ " + S.TEXT_NORMAL + line)
        continue
      }

      const hm = line.match(/^(#{1,6}) (.+)/)
      if (hm) {
        const title = inline(hm[2]!.replace(/\*\*/g, ""))
        out.push(hm[1]!.length === 1 ? chalk.bold.underline(title) : S.TEXT_NORMAL_BOLD + title + S.TEXT_NORMAL)
        continue
      }
      if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
        out.push(S.TEXT_DIM + "─".repeat(24) + S.TEXT_NORMAL)
        continue
      }
      const bullet = line.match(/^(\s*)[-*+] (.*)$/)
      if (bullet) {
        out.push(`${bullet[1]}${S.TEXT_DIM}•${S.TEXT_NORMAL} ${inline(bullet[2]!)}`)
        continue
      }
      const numbered = line.match(/^(\s*)(\d+)[.)] (.*)$/)
      if (numbered) {
        out.push(`${numbered[1]}${S.TEXT_DIM}${numbered[2]}.${S.TEXT_NORMAL} ${inline(numbered[3]!)}`)
        continue
      }
      const quote = line.match(/^>\s?(.*)$/)
      if (quote) {
        out.push(`${S.TEXT_DIM}▎${S.TEXT_NORMAL} ${inline(quote[1]!)}`)
        continue
      }
      out.push(inline(line))
    }

    return out.join("\n")
  }
}
