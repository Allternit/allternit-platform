import { existsSync, readFileSync, renameSync, unwatchFile, watchFile, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { basename, dirname, join } from 'node:path'

/**
 * The Desktop pet's settings file (electron-store `desktop-companion`, see
 * surfaces/allternit-desktop/src/main/desktop-companion.ts). The terminal pet
 * shares its `agentId` so both surfaces wear the same bot: we read it, write
 * it when the user picks a bot in the HUD, and watch it so a change made in
 * Desktop shows up here. Only `agentId` is ever touched; the pet's size,
 * position and visibility belong to Desktop.
 */
export function desktopPetSettingsPath(): string {
  if (process.env.GIZZI_DESKTOP_PET_FILE) return process.env.GIZZI_DESKTOP_PET_FILE
  const home = homedir()
  const base =
    process.platform === 'darwin'
      ? join(home, 'Library', 'Application Support')
      : process.platform === 'win32'
        ? (process.env.APPDATA ?? join(home, 'AppData', 'Roaming'))
        : (process.env.XDG_CONFIG_HOME ?? join(home, '.config'))
  return join(base, '@allternit', 'desktop', 'desktop-companion.json')
}

function readSettings(): Record<string, unknown> | undefined {
  try {
    const parsed = JSON.parse(readFileSync(desktopPetSettingsPath(), 'utf8'))
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : undefined
  } catch {
    return undefined
  }
}

/** The bot the Desktop pet wears, or undefined when Desktop has never run. */
export function readDesktopPetAgentId(): string | undefined {
  const id = readSettings()?.agentId
  return typeof id === 'string' && id.length > 0 && id.length <= 200 ? id : undefined
}

/**
 * Point the Desktop pet at a bot. Merges into Desktop's existing settings
 * (keeping everything else) and replaces the file atomically. Skipped when
 * Desktop isn't installed: there is no pet to sync with.
 */
export function writeDesktopPetAgentId(agentId: string): boolean {
  const path = desktopPetSettingsPath()
  if (!existsSync(dirname(path))) return false
  const next = { ...(readSettings() ?? {}), agentId }
  const temp = join(dirname(path), `.${basename(path)}.${process.pid}.tmp`)
  try {
    writeFileSync(temp, `${JSON.stringify(next, null, '\t')}\n`)
    renameSync(temp, path)
    return true
  } catch {
    return false
  }
}

/**
 * Call `onChange` when the Desktop pet switches bots. Watches the directory
 * (electron-store and our own writes replace the file, which drops a file
 * watch) and debounces the burst of events one save produces. Any event in
 * the folder triggers a re-read: when a temp file is renamed over the
 * target, macOS (Bun) reports only the temp file's name.
 */
export function watchDesktopPetAgentId(onChange: (agentId: string | undefined) => void): () => void {
  const path = desktopPetSettingsPath()
  let last = readDesktopPetAgentId()
  // Polls the path (stat), so Desktop replacing the file through a temp-file
  // rename is seen, and a missing file (Desktop not installed) is fine. Not
  // fs.watch: under Bun on macOS an unref'd fs.watch never delivers events,
  // and a ref'd one would keep gizzi alive.
  const listener = () => {
    const next = readDesktopPetAgentId()
    if (next !== last) {
      last = next
      onChange(next)
    }
  }
  const watcher = watchFile(path, { interval: 500, persistent: false }, listener)
  watcher.unref?.()
  return () => unwatchFile(path, listener)
}
