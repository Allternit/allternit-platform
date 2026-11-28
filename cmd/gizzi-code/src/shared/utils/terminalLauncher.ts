/**
 * Open a new terminal window that runs one command (D8, Gizzi overhaul).
 * Pure argv builders: nothing here runs a process. The window command only
 * ever carries the attach command and a session slug or id, never user text
 * (the seed travels as a task file). Closing the window detaches; it never
 * ends the session.
 */
import { basename } from "node:path"

export const TERMINALS = [
  "terminal",
  "iterm",
  "ghostty",
  "wt",
  "x-terminal-emulator",
  "gnome-terminal",
  "konsole",
  "kitty",
  "alacritty",
  "foot",
] as const
export type TerminalName = (typeof TERMINALS)[number]

export type Launch = {
  /** argv[0] is the program. No shell is involved when running it. */
  argv: string[]
  /** Working directory for the launcher process, where the terminal supports it that way. */
  cwd?: string
}

/** POSIX single-quote a word so a shell reads it back as one literal word. */
export function shellQuote(word: string): string {
  return /^[A-Za-z0-9_@%+=:,./-]+$/.test(word) ? word : `'${word.replace(/'/g, `'\\''`)}'`
}

/** AppleScript string-literal escape (the text goes between double quotes). */
export function appleScriptEscape(text: string): string {
  return text.replace(/\\/g, "\\\\").replace(/"/g, '\\"')
}

/** One shell line: optional `cd`, then the command. Only quoted words, no interpolation. */
function shellLine(command: string[], cwd?: string): string {
  const run = command.map(shellQuote).join(" ")
  return cwd ? `cd ${shellQuote(cwd)} && ${run}` : run
}

function isTerminalName(v: string): v is TerminalName {
  return (TERMINALS as readonly string[]).includes(v)
}

/** Map a program name or path ($TERMINAL, --terminal) onto a known terminal. */
export function normalizeTerminal(raw: string | undefined): TerminalName | undefined {
  if (!raw) return undefined
  const name = basename(raw.trim()).toLowerCase().replace(/\.(exe|app)$/, "")
  const alias: Record<string, TerminalName> = {
    apple_terminal: "terminal",
    iterm2: "iterm",
    "iterm.app": "iterm",
    windowsterminal: "wt",
    "windows-terminal": "wt",
  }
  const n = alias[name] ?? name
  return isTerminalName(n) ? n : undefined
}

export type PickArgs = {
  platform: NodeJS.Platform
  env: Record<string, string | undefined>
  /** `--terminal <name>`; wins over everything. */
  override?: string
  /** Is this program on PATH? Only asked on Linux. */
  has?: (program: string) => boolean
}

/** Terminal to open: override, then TERM_PROGRAM, then the OS default. */
export function pickTerminal(a: PickArgs): TerminalName {
  if (a.override) {
    const t = normalizeTerminal(a.override)
    if (!t) throw new Error(`Unknown terminal "${a.override}". Known: ${TERMINALS.join(", ")}.`)
    return t
  }
  if (a.platform === "darwin") {
    const t = normalizeTerminal(a.env.TERM_PROGRAM)
    return t === "iterm" || t === "ghostty" ? t : "terminal"
  }
  if (a.platform === "win32") return "wt"
  const fromEnv = normalizeTerminal(a.env.TERMINAL)
  if (fromEnv) return fromEnv
  const has = a.has ?? (() => false)
  const found = (["x-terminal-emulator", "gnome-terminal", "konsole", "kitty", "alacritty", "foot"] as const).find((p) => has(p))
  if (!found) throw new Error("No terminal found. Set $TERMINAL or pass --terminal <name>.")
  return found
}

/** The launch argv for `command` in a new window of `terminal`. */
export function buildLaunch(terminal: TerminalName, command: string[], cwd?: string): Launch {
  if (!command.length) throw new Error("Nothing to run in the window.")
  switch (terminal) {
    case "terminal": {
      const line = appleScriptEscape(shellLine(command, cwd))
      return { argv: ["osascript", "-e", 'tell application "Terminal" to activate', "-e", `tell application "Terminal" to do script "${line}"`] }
    }
    case "iterm": {
      const line = appleScriptEscape(shellLine(command, cwd))
      return {
        argv: [
          "osascript",
          "-e",
          'tell application "iTerm" to activate',
          "-e",
          `tell application "iTerm" to create window with default profile command "${line}"`,
        ],
      }
    }
    case "ghostty":
      return { argv: ["open", "-na", "Ghostty", "--args", ...(cwd ? [`--working-directory=${cwd}`] : []), "-e", ...command] }
    case "wt":
      return { argv: ["wt.exe", "new-tab", ...(cwd ? ["--startingDirectory", cwd] : []), "--", ...command] }
    case "x-terminal-emulator":
    case "konsole":
    case "alacritty":
      return { argv: [terminal, "-e", ...command], cwd }
    case "gnome-terminal":
      return { argv: ["gnome-terminal", "--", ...command], cwd }
    case "kitty":
    case "foot":
      return { argv: [terminal, ...command], cwd }
  }
}

/** Display form of a launch for `--dry-run` (quoted words; not run through a shell). */
export function formatLaunch(l: Launch): string {
  return (l.cwd ? `(cwd ${shellQuote(l.cwd)}) ` : "") + l.argv.map(shellQuote).join(" ")
}

/** The command a window runs to attach to a started session. Carries only an id. */
export function attachCommand(kind: "engine" | "terminal", id: string): string[] {
  return kind === "terminal" ? ["gizzi", "agents", "attach", id] : ["gizzi", "--session", id]
}
