/**
 * Shell Completions Command
 *
 * The single completion system for gizzi. The yargs `.completion()` built-in
 * that used to live in main.ts (as the separate `completion` command) was
 * removed in favor of this generator, which derives its output from the same
 * command registry main.ts registers (`src/cli/commands/registry.ts`), so
 * completions cannot drift from `gizzi --help`.
 *
 * Usage:
 *   gizzi completions bash    — print bash completion script
 *   gizzi completions zsh     — print zsh completion script
 *   gizzi completions fish    — print fish completion script
 *   gizzi completions install — install completions for detected shell
 */

import type { CommandModule } from "yargs"
import { UI } from "@/cli/ui"
import path from "path"
import os from "os"
import fs from "fs/promises"
import { existsSync } from "fs"
import { COMMANDS, type RegisteredCommand } from "@/cli/commands/registry"

const SCRIPT_NAME = "gizzi"

export interface CompletionEntry {
  name: string
  description: string
}

/**
 * Derive top-level command names and descriptions from the yargs command
 * tree. This is the "derives from the yargs command tree" guarantee: the
 * input is the exact array main.ts spreads into the root yargs instance.
 */
export function commandEntries(modules: RegisteredCommand[] = COMMANDS): CompletionEntry[] {
  const seen = new Set<string>()
  const entries: CompletionEntry[] = []
  for (const mod of modules) {
    const raw = Array.isArray(mod.command) ? mod.command[0] : mod.command
    if (typeof raw !== "string") continue
    // Hidden commands (describe: false) stay out of completions, as they
    // stay out of --help.
    if (mod.describe === false) continue
    const name = raw.split(/\s+/)[0]
    if (!name || name === "$0" || seen.has(name)) continue
    seen.add(name)
    entries.push({
      name,
      description: typeof mod.describe === "string" ? mod.describe : "",
    })
  }
  return entries
}

/**
 * Subcommands that the yargs tree models as `<action>` positionals or nested
 * builders inside a single module cannot be enumerated from the module tree
 * without running builders, so they are curated here and kept in sync with
 * the handlers in src/cli/commands/<group>/.
 */
const SUBCOMMAND_HINTS: Record<string, string[]> = {
  auth: ["login", "status", "logout", "diagnose", "profile"],
  session: ["list", "delete", "export"],
  config: ["list", "add", "remove", "set-active", "import", "export", "profile", "telemetry"],
  db: ["path", "migrate"],
  agents: [
    "up", "ps", "down", "whoami", "recover", "snapshot", "restore", "model", "handoff",
    "harness", "pack", "install", "templates", "wall", "attach", "doctor", "modes", "bot",
  ],
  orchestration: ["send", "capture", "transcript", "threads", "mail", "feed", "attention", "steer", "coordinate", "ac"],
  workflows: ["run", "drive", "template", "wake", "gate", "status"],
  workspace: ["campaign", "plan", "node", "approve", "proof", "judge", "board", "tasks", "team"],
  profile: ["list", "save", "activate", "deactivate", "delete", "show"],
  "permission-profile": ["list", "show", "activate", "deactivate", "save", "delete", "presets"],
  completions: ["bash", "zsh", "fish", "install"],
}

// Global options mirrored from the root yargs setup in main.ts.
const GLOBAL_OPTIONS: CompletionEntry[] = [
  { name: "help", description: "show help" },
  { name: "version", description: "show version number" },
  { name: "print-logs", description: "print logs to stderr" },
  { name: "log-level", description: "log level (DEBUG, INFO, WARN, ERROR)" },
  { name: "onboarding", description: "force the setup onboarding wizard" },
  { name: "ci", description: "run in CI/non-interactive mode" },
]

function bashQuoteWord(word: string): string {
  return word.replace(/[^a-zA-Z0-9_-]/g, "_")
}

function zshQuote(value: string): string {
  return value.replace(/'/g, `'\\''`).replace(/\n/g, " ")
}

function fishQuote(value: string): string {
  return value.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\$/g, "\\$").replace(/`/g, "\\`")
}

export function bashCompletions(modules: RegisteredCommand[] = COMMANDS): string {
  const entries = commandEntries(modules)
  const commands = entries.map((e) => e.name).join(" ")
  const cases = Object.entries(SUBCOMMAND_HINTS)
    .map(([group, subs]) => {
      const list = subs.map((s) => bashQuoteWord(s)).join(" ")
      return `    ${bashQuoteWord(group)})\n      COMPREPLY=( $(compgen -W "${list}" -- "\${cur}") )\n      ;;`
    })
    .join("\n")

  return `###-begin-gizzi-completions-###
#
# Gizzi Code shell completions (bash)
#
# Generated from the gizzi command registry — do not edit by hand.
#
# Installation:
#   gizzi completions bash >> ~/.bashrc
#   # or
#   gizzi completions bash > /etc/bash_completion.d/gizzi
#

_gizzi_completions() {
  local cur prev
  COMPREPLY=()
  cur="\${COMP_WORDS[COMP_CWORD]}"

  commands="${commands}"

  if [[ \${COMP_CWORD} -eq 1 ]]; then
    COMPREPLY=( $(compgen -W "\${commands}" -- "\${cur}") )
    return 0
  fi

  case "\${COMP_WORDS[1]}" in
${cases}
    *)
      COMPREPLY=( $(compgen -f -- "\${cur}") )
      ;;
  esac

  return 0
}

complete -F _gizzi_completions ${SCRIPT_NAME}
###-end-gizzi-completions-###
`
}

export function zshCompletions(modules: RegisteredCommand[] = COMMANDS): string {
  const entries = commandEntries(modules)
  const commandLines = entries
    .map((e) => `    '${zshQuote(e.name)}:${zshQuote(e.description)}'`)
    .join("\n")
  const cases = Object.entries(SUBCOMMAND_HINTS)
    .map(([group, subs]) => {
      const lines = subs.map((s) => `          '${zshQuote(s)}'`).join("\n")
      return `        ${bashQuoteWord(group)})\n          local -a subs\n          subs=(\n${lines}\n          )\n          _describe -t subcommands '${zshQuote(group)} subcommand' subs\n          ;;`
    })
    .join("\n")

  return `#compdef gizzi

# Gizzi Code shell completions (zsh)
#
# Generated from the gizzi command registry — do not edit by hand.
#
# Installation:
#   gizzi completions zsh > ~/.zsh/completions/_gizzi
#   # Ensure ~/.zsh/completions is in fpath:
#   # fpath=(~/.zsh/completions $fpath)
#   # Then: compinit
#

_gizzi() {
  local -a commands
  commands=(
${commandLines}
  )

  _arguments -C \\
    '1:command:->command' \\
    '*::arg:->args'

  case $state in
    command)
      _describe -t commands 'gizzi command' commands
      ;;
    args)
      case $words[1] in
${cases}
      esac
      ;;
  esac
}

_gizzi "$@"
`
}

export function fishCompletions(modules: RegisteredCommand[] = COMMANDS): string {
  const entries = commandEntries(modules)
  const lines: string[] = [
    `# Gizzi Code shell completions (fish)`,
    `#`,
    `# Generated from the gizzi command registry — do not edit by hand.`,
    `#`,
    `# Installation:`,
    `#   gizzi completions fish > ~/.config/fish/completions/gizzi.fish`,
    ``,
    `# Disable file completions by default`,
    `complete -c gizzi -f`,
    ``,
    `# Top-level commands`,
  ]
  for (const e of entries) {
    lines.push(`complete -c gizzi -n "__fish_use_subcommand" -a ${e.name} -d "${fishQuote(e.description)}"`)
  }
  lines.push(``, `# Global options`)
  for (const o of GLOBAL_OPTIONS) {
    lines.push(`complete -c gizzi -l ${o.name} -d "${fishQuote(o.description)}"`)
  }
  for (const [group, subs] of Object.entries(SUBCOMMAND_HINTS)) {
    lines.push(``, `# ${group} subcommands`)
    for (const s of subs) {
      lines.push(`complete -c gizzi -n "__fish_seen_subcommand_from ${group}" -a ${s}`)
    }
  }
  return lines.join("\n") + "\n"
}

async function installCompletions(shell: string): Promise<void> {
  const home = os.homedir()

  switch (shell) {
    case "bash": {
      const rcPath = path.join(home, ".bashrc")
      if (!existsSync(rcPath)) {
        UI.error("~/.bashrc not found. Run `gizzi completions bash` and add manually.")
        return
      }
      const content = await fs.readFile(rcPath, "utf8")
      if (content.includes("###-begin-gizzi-completions-###")) {
        UI.info("Bash completions already installed.")
        return
      }
      await fs.appendFile(rcPath, "\n" + bashCompletions())
      UI.success("Bash completions appended to ~/.bashrc. Restart your shell or run `source ~/.bashrc`.")
      break
    }

    case "zsh": {
      const compDir = path.join(home, ".zsh", "completions")
      await fs.mkdir(compDir, { recursive: true })
      const filePath = path.join(compDir, "_gizzi")
      await fs.writeFile(filePath, zshCompletions())
      UI.success(`Zsh completions written to ${filePath}.`)
      UI.info("Ensure this directory is in fpath, then run `compinit`.")
      break
    }

    case "fish": {
      const compDir = path.join(home, ".config", "fish", "completions")
      await fs.mkdir(compDir, { recursive: true })
      const filePath = path.join(compDir, "gizzi.fish")
      await fs.writeFile(filePath, fishCompletions())
      UI.success(`Fish completions written to ${filePath}.`)
      break
    }

    default:
      UI.error(`Unknown shell: ${shell}`)
  }
}

function detectShell(): string | null {
  const shell = process.env.SHELL ?? ""
  if (shell.includes("zsh")) return "zsh"
  if (shell.includes("bash")) return "bash"
  if (shell.includes("fish")) return "fish"
  return null
}

export const CompletionsCommand: CommandModule = {
  command: "completions <shell>",
  describe: "generate or install shell completions (bash, zsh, fish)",
  builder: (yargs) =>
    yargs.positional("shell", {
      describe: "shell type",
      choices: ["bash", "zsh", "fish", "install"],
      type: "string",
      demandOption: true,
    }),
  handler: async (argv) => {
    const shell = argv.shell as string

    try {
      switch (shell) {
        case "bash":
          process.stdout.write(bashCompletions())
          break
        case "zsh":
          process.stdout.write(zshCompletions())
          break
        case "fish":
          process.stdout.write(fishCompletions())
          break
        case "install": {
          const detected = detectShell()
          if (!detected) {
            UI.error("Could not detect shell. Specify explicitly: gizzi completions bash|zsh|fish")
            process.exit(1)
          }
          UI.info(`Detected shell: ${detected}`)
          await installCompletions(detected)
          break
        }
        default:
          UI.error(`Unknown shell: ${shell}`)
          process.exit(1)
      }
    } catch (e) {
      UI.error(e instanceof Error ? e.message : String(e))
      process.exit(1)
    }
  },
}
