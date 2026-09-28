import fs from "fs/promises"
import path from "path"
import { Filesystem } from "@/shared/util/filesystem"
import { Log } from "@/shared/util/log"
import { GlobalPaths } from "./paths"

const log = () => Log.create({ service: "global" })

const legacyApp = "gizzi-code"
const legacyData = path.join(GlobalPaths.data, legacyApp)
const legacyConfig = path.join(GlobalPaths.config, legacyApp)
// Before the gizzi-code rename, preferences, themes and config lived in
// ~/.config/gizzi. Copied forward once (never overwritten, never deleted).
const preRenameConfig = path.join(path.dirname(GlobalPaths.config), "gizzi")

export namespace Global {
  export const Path = GlobalPaths
}

async function copyLegacyFile(input: { from: string; to: string; mode?: number }) {
  if (!(await Filesystem.exists(input.from))) return
  if (await Filesystem.exists(input.to)) return
  await fs.mkdir(path.dirname(input.to), { recursive: true })
  await fs.copyFile(input.from, input.to)
  if (input.mode !== undefined) {
    // chmod hardening is best-effort; copyFile above already succeeded.
    await fs.chmod(input.to, input.mode).catch(() => {})
  }
}

const CACHE_VERSION = "21"

export async function init() {
  await Promise.all([
    fs.mkdir(Global.Path.data, { recursive: true }),
    fs.mkdir(Global.Path.cache, { recursive: true }),
    fs.mkdir(Global.Path.config, { recursive: true }),
    fs.mkdir(Global.Path.state, { recursive: true }),
    fs.mkdir(Global.Path.log, { recursive: true }),
    fs.mkdir(Global.Path.bin, { recursive: true }),
  ])

  await Promise.all([
    copyLegacyFile({
      from: path.join(legacyConfig, "config.json"),
      to: path.join(Global.Path.config, "config.json"),
    }),
    copyLegacyFile({
      from: path.join(legacyConfig, "gizziio.json"),
      to: path.join(Global.Path.config, "gizzi.json"),
    }),
    copyLegacyFile({
      from: path.join(legacyConfig, "gizziio.jsonc"),
      to: path.join(Global.Path.config, "gizzi.jsonc"),
    }),
    ...["gizzi.json", "gizzi.jsonc", "preferences.json", "theme.json", "themes.json"].map((file) =>
      copyLegacyFile({
        from: path.join(preRenameConfig, file),
        to: path.join(Global.Path.config, file),
      }),
    ),
    copyLegacyFile({
      from: path.join(legacyData, "auth.json"),
      to: path.join(Global.Path.data, "auth.json"),
      mode: 0o600,
    }),
    copyLegacyFile({
      from: path.join(legacyData, "mcp-auth.json"),
      to: path.join(Global.Path.data, "mcp-auth.json"),
      mode: 0o600,
    }),
  ])

  const version = await Filesystem.readText(path.join(Global.Path.cache, "version")).catch(() => "0")

  if (version !== CACHE_VERSION) {
    try {
      const contents = await fs.readdir(Global.Path.cache)
      await Promise.all(
        contents.map((item) =>
          fs.rm(path.join(Global.Path.cache, item), {
            recursive: true,
            force: true,
          }),
        ),
      )
    } catch (e) {
      log().warn("Failed to clear cache directory", { error: e })
    }
    await Filesystem.write(path.join(Global.Path.cache, "version"), CACHE_VERSION)
  }
}

export interface GlobalContext {
  debug: boolean
  [key: string]: unknown
}

const context: GlobalContext = {
  debug: process.env.GIZZI_DEBUG === 'true' || process.env.GIZZI_DEBUG === '1',
}

/** Process-wide context flags (debug, feature toggles). */
export function getGlobalContext(): GlobalContext {
  return context
}
