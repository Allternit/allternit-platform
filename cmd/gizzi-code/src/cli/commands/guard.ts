import { existsSync } from "node:fs"
import { homedir } from "node:os"
import { join } from "node:path"
import { spawn } from "node:child_process"
import { cmd } from "@/cli/commands/cmd"

/** Guard owns authentication, run state and authorization. Forward argv without a shell. */
export const GuardCommand = cmd({
  command: "guard [args..]",
  describe: "open Guard's case TUI or use its shared conversations, runs and artifacts",
  builder: (yargs) => yargs.strict(false).parserConfiguration({ "unknown-options-as-args": true }),
  handler: async () => {
    const index = process.argv.indexOf("guard")
    const args = process.argv.slice(index + 1)
    const local = join(homedir(), ".local", "bin", "allternit-guard")
    const child = spawn(existsSync(local) ? local : "allternit-guard", args.length ? args : ["tui"], { stdio: "inherit", shell: false })
    process.exitCode = await new Promise<number>((resolve) => {
      child.once("error", (error) => {
        process.stderr.write(`Guard could not start: ${error.message}. Install the Allternit Guard CLI first.\n`)
        resolve(1)
      })
      child.once("exit", (code) => resolve(code ?? 1))
    })
  },
})
