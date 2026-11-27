/**
 * `gizzi start --list` / `gizzi start --device <d> --folder <path> …` — start
 * a session on this or another of your computers (Start anywhere, D8). Same
 * code and guards as the StartSession tool, so other CLIs can run it from
 * their shell. Depth and the mode cap come from the session this runs in
 * (ALLTERNIT_SESSION_START_DEPTH / ALLTERNIT_SESSION_START_MODE); a person at
 * a terminal is not capped, a shell with no terminal is capped at "default".
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { callerStartMode, checkStart, isStartMode, START_MODES } from "@/cli/work/start-guards"
import { formatStartDevices, listStartDevices, parentDepth, startSession } from "@/cli/work/start-session"

export const StartCommand = cmd({
  command: "start",
  describe: "start a session on this or another computer (--list shows your computers)",
  builder: (y: Argv) =>
    y
      .option("list", { type: "boolean", describe: "list your computers" })
      .option("device", { type: "string", describe: "computer name or id; default this computer" })
      .option("folder", { type: "string", describe: "folder path on that computer" })
      .option("runtime", { type: "string", describe: "coding tool (default allternit)" })
      .option("mode", { type: "string", choices: [...START_MODES], describe: "permission mode" })
      .option("prompt", { type: "string", describe: "first task for the session" })
      .option("json", { type: "boolean", describe: "print JSON" }),
  handler: async (args) => {
    try {
      if (args.list) {
        const devices = await listStartDevices()
        process.stdout.write((args.json ? JSON.stringify(devices, null, 2) : formatStartDevices(devices)) + "\n")
        return
      }
      if (!args.folder) {
        UI.error("Give --folder <path> (or --list to see your computers).")
        process.exitCode = 1
        return
      }
      const envMode = process.env.ALLTERNIT_SESSION_START_MODE
      const callerMode = isStartMode(envMode) ? envMode : process.stdin.isTTY ? "bypass" : callerStartMode("default")
      const guard = checkStart({
        callerMode,
        requestedMode: isStartMode(args.mode) ? args.mode : undefined,
        parentDepth: parentDepth(),
      })
      if (!guard.ok) {
        UI.error(guard.message)
        process.exitCode = 1
        return
      }
      const result = await startSession({
        device: args.device,
        folder: args.folder,
        runtime: args.runtime,
        mode: guard.mode,
        prompt: args.prompt,
        depth: guard.depth,
      })
      process.stdout.write((args.json ? JSON.stringify(result, null, 2) : result.line) + "\n")
    } catch (e) {
      UI.error((e as Error).message)
      process.exitCode = 1
    }
  },
})
