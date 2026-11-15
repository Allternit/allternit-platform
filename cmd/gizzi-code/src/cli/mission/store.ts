/**
 * Mission records on disk: `<repo>/.allternit/missions/<id>.json`, written
 * atomically, so `gizzi mission resume <id>` picks up after a restart and the
 * TUI badge, `ls` and `status` read the same state the runner writes.
 */
import fs from "node:fs"
import path from "node:path"
import { spawnSync } from "node:child_process"
import type { Mission } from "@/cli/mission/state"

/** The repo root for `cwd` (missions live with the project), or cwd outside git. */
export function projectRoot(cwd: string): string {
  const r = spawnSync("git", ["rev-parse", "--show-toplevel"], { cwd, encoding: "utf8" })
  const top = r.status === 0 ? r.stdout.trim() : cwd
  // Inside a mission or swarm worktree, keep using the main checkout's records.
  const common = spawnSync("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], { cwd, encoding: "utf8" })
  if (common.status === 0) {
    const dir = common.stdout.trim()
    if (path.basename(dir) === ".git") return path.dirname(dir)
  }
  return top
}

export function missionsDir(root: string): string {
  return path.join(root, ".allternit", "missions")
}

export function newMissionId(now = Date.now()): string {
  return `mi-${now.toString(36).slice(-5)}${Math.floor(Math.random() * 36).toString(36)}`
}

export function readMission(root: string, id: string): Mission | null {
  try {
    return JSON.parse(fs.readFileSync(path.join(missionsDir(root), `${id}.json`), "utf8")) as Mission
  } catch {
    return null
  }
}

export function writeMission(m: Mission): void {
  const dir = missionsDir(m.root)
  fs.mkdirSync(dir, { recursive: true })
  const file = path.join(dir, `${m.id}.json`)
  const tmp = `${file}.${process.pid}.tmp`
  fs.writeFileSync(tmp, JSON.stringify(m, null, 2))
  fs.renameSync(tmp, file)
}

/** Every mission of this project, newest first. */
export function listMissions(root: string): Mission[] {
  let names: string[] = []
  try {
    names = fs.readdirSync(missionsDir(root)).filter((n) => n.endsWith(".json"))
  } catch {
    return []
  }
  return names
    .map((n) => readMission(root, n.replace(/\.json$/, "")))
    .filter((m): m is Mission => !!m)
    .sort((a, b) => b.createdAt - a.createdAt)
}

/** The newest mission that is running (for the footer badge). */
export function activeMission(root: string): Mission | null {
  return listMissions(root).find((m) => m.status === "running") ?? null
}

export function runnerAlive(m: Mission): boolean {
  if (!m.runnerPid) return false
  try {
    process.kill(m.runnerPid, 0)
    return true
  } catch {
    return false
  }
}

export function logFile(m: Pick<Mission, "root" | "id">): string {
  return path.join(missionsDir(m.root), `${m.id}.log`)
}
