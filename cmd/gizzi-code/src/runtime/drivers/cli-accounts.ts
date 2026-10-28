import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { CLI_CATALOG } from "@/runtime/providers/discovery/subprocess"

/**
 * Extra signed-in accounts for a coding CLI. allternit-api keeps the rows
 * (`cli_accounts`, V279) and gives each added account its own home under
 * `~/.allternit/cli-homes/<id>`; the CLI is pointed at it with the catalog's
 * `accountHomeEnv` (CLAUDE_CONFIG_DIR, CODEX_HOME). The default account sets
 * nothing: it keeps the CLI's stock home, and for Claude on macOS that is the
 * keychain entry bot turns rely on (runtime/bots/bot-turn.ts).
 */
export namespace CliAccounts {
  export function root(): string {
    return path.join(os.homedir(), ".allternit", "cli-homes")
  }

  /** An id allternit-api minted: one path segment, nothing that climbs out of the root. */
  function safe(id: string): boolean {
    return /^[A-Za-z0-9_-]{1,64}$/.test(id)
  }

  /** The user's default account per CLI id, as allternit-api last wrote it. */
  function defaults(): Record<string, string> {
    try {
      return JSON.parse(fs.readFileSync(path.join(root(), "defaults.json"), "utf8"))
    } catch {
      return {}
    }
  }

  /**
   * The env that puts a CLI turn on its account, or undefined for the stock
   * home. Order: the request's account, the bot's account, the user's default.
   * "default" (explicit) means the stock home even when another account is the
   * user's default.
   */
  export function env(cliName: string, requested?: string, bot?: string): Record<string, string> | undefined {
    const tool = CLI_CATALOG.find((t) => t.id === cliName || t.bin === cliName || t.aliases?.includes(cliName))
    if (!tool?.accountHomeEnv) return undefined
    const id = requested || bot || defaults()[tool.id]
    if (!id || id === "default" || !safe(id)) return undefined
    const home = path.join(root(), id)
    if (!fs.existsSync(home) || !belongsTo(home, tool.id)) return undefined
    return { [tool.accountHomeEnv]: home }
  }

  /**
   * Whether an account home is this CLI's: allternit-api writes the CLI id to
   * `.allternit-cli-tool` when it makes the home. A bot's account can be
   * another CLI's after its brain switched (Claude → Codex); that one is
   * skipped, never handed to the wrong CLI.
   */
  function belongsTo(home: string, toolId: string): boolean {
    try {
      return fs.readFileSync(path.join(home, ".allternit-cli-tool"), "utf8").trim() === toolId
    } catch {
      return true
    }
  }
}
