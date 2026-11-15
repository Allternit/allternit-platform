import { existsSync } from "fs"
import { EOL } from "os"
import { Config } from "@/runtime/context/config/config"
import { Instance } from "@/runtime/context/project/instance"
import { Provider } from "@/runtime/providers/provider"
import { currentDefaultBrain } from "@/runtime/providers/default-brain"
import { getDefaultMainLoopModelSetting, getUserSpecifiedModelSetting } from "@/cli/ui/ink-app/utils/model/model"
import { cmd } from "@/cli/commands/cmd"

type Trio = { model?: unknown; model_auto?: unknown; small_model?: unknown }

function trio(config: Record<string, any> | undefined): Trio {
  return { model: config?.model, model_auto: config?.model_auto, small_model: config?.small_model }
}

function show(value: unknown): string {
  return value === undefined || value === null ? "unset" : String(value)
}

function trioLine(t: Trio): string {
  return `model=${show(t.model)} model_auto=${show(t.model_auto)} small_model=${show(t.small_model)}`
}

function refOf(value: unknown): string | null {
  if (!value) return null
  if (typeof value === "string") return value
  const v = value as { providerID?: string; modelID?: string }
  return v.providerID && v.modelID ? `${v.providerID}/${v.modelID}` : String(value)
}

export const ModelCommand = cmd({
  command: "model",
  describe: "show where the default model comes from (config, resolver, runtime, TUI)",
  builder: (yargs) =>
    yargs.option("json", { describe: "print as one JSON object", type: "boolean", default: false }),
  async handler(args) {
    await Instance.provide({
      directory: process.cwd(),
      async fn() {
        const files = [...Config.globalConfigFiles(), ...Config.configDirOverrideFiles()].map((path) => {
          const present = existsSync(path)
          let values: Trio = {}
          if (present) {
            try {
              const text = require("fs").readFileSync(path, "utf8") as string
              values = path.endsWith(".toml")
                ? trio(Bun.TOML.parse(text) as Record<string, any>)
                : trio(JSON.parse(text.replace(/^\s*\/\/.*$/gm, "")))
            } catch {}
          }
          return { path, present, ...values }
        })

        const merged = trio(await Config.get().catch(() => undefined))
        const global = trio(Config.readGlobalConfigSync())
        const brain = await currentDefaultBrain().catch((e) => ({
          model: null as string | null,
          reason: e instanceof Error ? e.message : String(e),
          pinned: false,
        }))

        let runtime: string | null = null
        let runtimeError: string | undefined
        try {
          runtime = refOf(await Provider.defaultModel())
        } catch (e) {
          runtimeError = (e instanceof Error ? e.message || e.name : String(e)) || "unknown error"
        }

        let tui: string | null = null
        let tuiError: string | undefined
        try {
          // What the TUI runs: the user's pick when there is one, else its default.
          tui = refOf(getUserSpecifiedModelSetting() ?? getDefaultMainLoopModelSetting())
        } catch (e) {
          tuiError = e instanceof Error ? e.message : String(e)
        }

        const match = Boolean(brain.model && runtime && tui && brain.model === runtime && runtime === tui)

        if (args.json) {
          process.stdout.write(
            JSON.stringify(
              {
                configFiles: files,
                merged,
                global,
                resolver: brain,
                runtime: { model: runtime, error: runtimeError },
                tui: { model: tui, error: tuiError },
                match,
              },
              null,
              2,
            ) + EOL,
          )
          return
        }

        const out: string[] = ["config files:"]
        for (const f of files) {
          const set = [
            f.model !== undefined && `model=${show(f.model)}`,
            f.model_auto !== undefined && `model_auto=${show(f.model_auto)}`,
            f.small_model !== undefined && `small_model=${show(f.small_model)}`,
          ].filter(Boolean)
          out.push(`  ${f.path} (${f.present ? "present" : "missing"})${set.length ? " " + set.join(" ") : ""}`)
        }
        out.push(`merged: ${trioLine(merged)}`)
        out.push(`global: ${trioLine(global)}`)
        out.push(`resolver: ${brain.model ?? "none"} (pinned=${brain.pinned}; ${brain.reason})`)
        out.push(`runtime default: ${runtime ?? `error: ${runtimeError}`}`)
        out.push(`tui model: ${tui ?? `error: ${tuiError}`}`)
        out.push(`match: ${match ? "yes" : "no"}`)
        process.stdout.write(out.join(EOL) + EOL)
      },
    })
  },
})
