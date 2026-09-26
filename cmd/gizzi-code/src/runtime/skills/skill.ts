import z from "zod/v4"
import path from "path"
import os from "os"
import { Config } from "@/runtime/context/config/config"
import { Instance } from "@/runtime/context/project/instance"
import { State } from "@/runtime/context/project/state"
import { NamedError } from "@allternit/gizzi-util/error.js"
import { ConfigMarkdown } from "@/runtime/context/config/markdown"
import { Log } from "@/shared/util/log"
import { Global } from "@/runtime/context/global"
import { Filesystem } from "@/shared/util/filesystem"
import { Flag } from "@/runtime/context/flag/flag"
import { Bus } from "@/shared/bus"
import { Session } from "@/runtime/session"
import { Discovery } from "@/runtime/skills/discovery"
import { Glob } from "@/shared/util/glob"
import { BUNDLED_SKILLS } from "@/runtime/skills/bundledSkills"
import { conditionalSkillDirectories } from "@/runtime/skills/loadSkillsDir"
import ignore from "ignore"

/**
 * Deterministic skill catalog. Priority follows the Kimi Code catalog model:
 * project > user > configured/remote > built-in. Equal-priority roots retain
 * their declared scan order, and every losing definition remains inspectable.
 */
export namespace Skill {
  const log = Log.create({ service: "skill" })
  const MAX_SCAN_DEPTH = 8
  const activatedConditional = new Map<string, Set<string>>()

  export const Source = z.enum(["builtin", "extra", "remote", "user", "project"])
  export type Source = z.infer<typeof Source>

  export const PRIORITY: Readonly<Record<Source, number>> = {
    builtin: 0,
    remote: 10,
    extra: 10,
    user: 20,
    project: 30,
  }

  export const Info = z.object({
    name: z.string(),
    description: z.string(),
    location: z.string(),
    content: z.string(),
    source: Source.default("extra"),
    priority: z.number().int().default(10),
    root: z.string().optional(),
    metadata: z.record(z.string(), z.unknown()).default({}),
    builtin: z.boolean().default(false),
    paths: z.string().array().optional(),
    conditional: z.boolean().default(false),
  })
  export type Info = z.infer<typeof Info>

  export const Collision = z.object({
    name: z.string(),
    winner: Info,
    shadowed: Info.array(),
  })
  export type Collision = z.infer<typeof Collision>

  export const InvalidError = NamedError.create(
    "SkillInvalidError",
    z.object({
      path: z.string(),
      message: z.string().optional(),
      issues: z.custom<z.core.$ZodIssue[]>().optional(),
    }),
  )

  export const NameMismatchError = NamedError.create(
    "SkillNameMismatchError",
    z.object({ path: z.string(), expected: z.string(), actual: z.string() }),
  )

  type Candidate = Info & { order: number }
  type Root = { path: string; source: Source; label: string }

  const scan = async () => {
    const candidates: Candidate[] = []
    const scannedRoots: Root[] = []
    let order = 0

    const emitInvalid = (location: string, err: unknown) => {
      const message = ConfigMarkdown.FrontmatterError.isInstance(err)
        ? (err as { data?: { message?: string } }).data?.message ?? `Failed to parse skill ${location}`
        : `Failed to parse skill ${location}`
      Bus.publish(Session.Event.Error, { error: { name: "SkillInvalidError", message, data: { path: location } } })
      log.error("failed to load skill", { skill: location, err })
    }

    const parseFile = async (location: string, root: Root): Promise<Candidate | undefined> => {
      const md = await ConfigMarkdown.parse(location).catch((err) => {
        emitInvalid(location, err)
        return undefined
      })
      if (!md) return
      const parsed = Info.pick({ name: true, description: true }).safeParse(md.data)
      if (!parsed.success) {
        log.warn("skipping skill with invalid frontmatter", { location, issues: parsed.error.issues })
        return
      }
      return {
        ...parsed.data,
        location,
        content: md.content,
        source: root.source,
        priority: PRIORITY[root.source],
        root: root.path,
        metadata: md.data as Record<string, unknown>,
        builtin: false,
        paths: parsePaths((md.data as Record<string, unknown>).paths),
        conditional: parsePaths((md.data as Record<string, unknown>).paths) !== undefined,
        order: order++,
      }
    }

    const scanRoot = async (root: Root) => {
      if (!(await Filesystem.isDir(root.path))) return
      scannedRoots.push(root)
      const files = await Glob.scan("**/SKILL.md", {
        cwd: root.path,
        absolute: true,
        include: "file",
        dot: false,
        symlink: true,
      }).catch((error) => {
        log.error("failed to scan skill root", { root: root.path, error })
        return []
      })
      const flat = await Glob.scan("*.md", {
        cwd: root.path,
        absolute: true,
        include: "file",
        symlink: true,
      }).catch(() => [])
      const parsed = new Map<string, Candidate>()
      for (const location of [...new Set([...files, ...flat])].toSorted((a, b) => {
        const depth = relativeDepth(root.path, a) - relativeDepth(root.path, b)
        return depth || a.localeCompare(b)
      })) {
        const depth = relativeDepth(root.path, location)
        if (depth > MAX_SCAN_DEPTH) {
          log.warn("skipping skill beyond scan depth", { location, maxDepth: MAX_SCAN_DEPTH })
          continue
        }
        const candidate = await parseFile(location, root)
        if (!candidate) continue
        const parent = nearestParent(parsed, location)
        if (parent) {
          if (!hasSubSkills(parent.metadata)) {
            log.warn("skipping nested skill because parent has not enabled sub-skills", {
              location,
              parent: parent.location,
            })
            continue
          }
          candidate.name = qualify(parent.name, candidate.name)
          candidate.metadata = { ...candidate.metadata, isSubSkill: true }
        } else if (depth > 1) {
          // Nested payload directories are not independent skill roots.
          continue
        }
        parsed.set(location, candidate)
        candidates.push(candidate)
      }
    }

    if (!disableBuiltins()) {
      for (const bundled of BUNDLED_SKILLS) {
        candidates.push({
          name: bundled.name,
          description: bundled.description,
          location: `builtin://${bundled.name}`,
          content: bundled.content,
          source: "builtin",
          priority: PRIORITY.builtin,
          root: "builtin://",
          metadata: bundled.metadata ?? {},
          builtin: true,
          conditional: false,
          order: order++,
        })
      }
    }

    if (!Flag.GIZZI_DISABLE_EXTERNAL_SKILLS) {
      // ".allternit" is where the Allternit app writes skills (Customize → Skills).
      for (const brand of [".claude", ".agents", ".openclaw", ".allternit"]) {
        await scanRoot({ path: path.join(Global.Path.home, brand, "skills"), source: "user", label: brand })
      }
      for await (const found of Filesystem.up({
        targets: [".claude", ".agents", ".openclaw"],
        start: Instance.directory,
        stop: Instance.worktree,
      })) {
        await scanRoot({ path: path.join(found, "skills"), source: "project", label: path.basename(found) })
      }
    }

    for (const dir of await Config.directories()) {
      const source: Source = isWithin(Instance.worktree, dir) ? "project" : "user"
      await scanRoot({ path: path.join(dir, "skill"), source, label: "gizzi" })
      await scanRoot({ path: path.join(dir, "skills"), source, label: "gizzi" })
    }

    const config = await Config.get()
    for (const skillPath of config.skills?.paths ?? []) {
      const expanded = skillPath.startsWith("~/") ? path.join(os.homedir(), skillPath.slice(2)) : skillPath
      const resolved = path.isAbsolute(expanded) ? expanded : path.join(Instance.directory, expanded)
      await scanRoot({ path: resolved, source: "extra", label: "configured" })
    }

    for (const url of config.skills?.urls ?? []) {
      for (const dir of await Discovery.pull(url)) {
        await scanRoot({ path: dir, source: "remote", label: url })
      }
    }

    for (const entry of conditionalSkillDirectories()) {
      const stat = await Filesystem.isDir(entry.path)
      if (!stat) continue
      const direct = path.join(entry.path, "SKILL.md")
      if (await Filesystem.exists(direct)) {
        const root = { path: entry.path, source: "extra" as const, label: "conditional" }
        scannedRoots.push(root)
        const candidate = await parseFile(direct, root)
        if (candidate) candidates.push(candidate)
      } else {
        await scanRoot({ path: entry.path, source: "extra", label: "conditional" })
      }
    }

    const groups = new Map<string, Candidate[]>()
    for (const candidate of candidates) {
      const key = candidate.name.toLowerCase()
      groups.set(key, [...(groups.get(key) ?? []), candidate])
    }
    const skills: Record<string, Info> = {}
    const collisions: Collision[] = []
    for (const entries of groups.values()) {
      entries.sort((a, b) => b.priority - a.priority || a.order - b.order || a.location.localeCompare(b.location))
      const [winner, ...shadowed] = entries
      skills[winner.name] = withoutOrder(winner)
      if (shadowed.length) {
        collisions.push({ name: winner.name, winner: withoutOrder(winner), shadowed: shadowed.map(withoutOrder) })
        log.warn("skill collision resolved", {
          name: winner.name,
          winner: winner.location,
          shadowed: shadowed.map((item) => item.location),
        })
      }
    }

    return {
      skills,
      collisions: collisions.toSorted((a, b) => a.name.localeCompare(b.name)),
      roots: scannedRoots,
      dirs: [...new Set(Object.values(skills).filter((item) => !item.builtin).map((item) => path.dirname(item.location)))],
    }
  }

  export const state = Instance.state(scan)

  /** Rescan skill roots (after a skill is installed or removed on disk). */
  export function reload() {
    State.forget(scan)
  }

  export async function get(name: string) {
    const catalog = await state()
    const dynamic = await dynamicSkills()
    const skill = dynamic.find((item) => item.name.toLowerCase() === name.toLowerCase()) ??
      catalog.skills[name] ?? Object.values(catalog.skills).find((item) => item.name.toLowerCase() === name.toLowerCase())
    return skill && isActive(skill) ? skill : undefined
  }

  export async function all() {
    const catalog = await state()
    const merged = new Map(Object.values(catalog.skills).map((item) => [item.name.toLowerCase(), item]))
    for (const item of await dynamicSkills()) merged.set(item.name.toLowerCase(), item)
    return [...merged.values()].filter(isActive).toSorted((a, b) => a.name.localeCompare(b.name))
  }

  export async function dirs() {
    return state().then((catalog) => catalog.dirs)
  }

  export async function collisions() {
    return state().then((catalog) => catalog.collisions)
  }

  export async function roots() {
    return state().then((catalog) => catalog.roots)
  }

  /** Activate path-scoped skills for this project. Returns newly visible names. */
  export async function activateForPaths(filePaths: string[]) {
    const key = Instance.worktree
    const active = activatedConditional.get(key) ?? new Set<string>()
    activatedConditional.set(key, active)
    const catalog = await state()
    const activated: string[] = []
    for (const skill of [...Object.values(catalog.skills), ...await dynamicSkills()]) {
      if (!skill.conditional || !skill.paths?.length || active.has(skill.name.toLowerCase())) continue
      const matcher = ignore().add(skill.paths)
      const matched = filePaths.some((file) => {
        const relative = path.isAbsolute(file) ? path.relative(Instance.worktree, file) : file
        if (!relative || relative.startsWith("..") || path.isAbsolute(relative)) return false
        return matcher.ignores(relative)
      })
      if (matched) {
        active.add(skill.name.toLowerCase())
        activated.push(skill.name)
      }
    }
    return activated
  }

  function withoutOrder(candidate: Candidate): Info {
    const { order: _order, ...info } = candidate
    return info
  }

  function relativeDepth(root: string, location: string) {
    const rel = path.relative(root, location)
    if (path.basename(location) === "SKILL.md") return Math.max(1, rel.split(path.sep).length - 1)
    return 1
  }

  function nearestParent(parsed: Map<string, Candidate>, location: string) {
    let current = path.dirname(path.dirname(location))
    while (current !== path.dirname(current)) {
      const candidate = parsed.get(path.join(current, "SKILL.md"))
      if (candidate) return candidate
      current = path.dirname(current)
    }
  }

  function hasSubSkills(metadata: Record<string, unknown>) {
    const nested = metadata.metadata
    return metadata["has-sub-skill"] === true || metadata.hasSubSkill === true ||
      (typeof nested === "object" && nested !== null &&
        ((nested as Record<string, unknown>)["has-sub-skill"] === true ||
          (nested as Record<string, unknown>).hasSubSkill === true))
  }

  function qualify(parent: string, child: string) {
    return child === parent || child.startsWith(`${parent}.`) ? child : `${parent}.${child}`
  }

  function isWithin(parent: string, child: string) {
    const rel = path.relative(path.resolve(parent), path.resolve(child))
    return rel === "" || (!rel.startsWith("..") && !path.isAbsolute(rel))
  }

  function disableBuiltins() {
    return [process.env.GIZZI_DISABLE_BUILTIN_SKILLS, process.env.Allternit_DISABLE_BUILTIN_SKILLS]
      .some((value) => value === "1" || value?.toLowerCase() === "true")
  }

  function isActive(skill: Info) {
    return !skill.conditional || activatedConditional.get(Instance.worktree)?.has(skill.name.toLowerCase()) === true
  }

  function parsePaths(value: unknown): string[] | undefined {
    const values = Array.isArray(value) ? value : typeof value === "string" ? value.split(/[\n,]/) : []
    const paths = values.map((item) => String(item).trim()).filter((item) => item && item !== "**")
    return paths.length ? paths : undefined
  }

  async function dynamicSkills(): Promise<Info[]> {
    const result: Info[] = []
    for (const entry of conditionalSkillDirectories()) {
      const root = entry.path
      const direct = path.join(root, "SKILL.md")
      const locations = await Filesystem.exists(direct)
        ? [direct]
        : await Glob.scan("**/SKILL.md", { cwd: root, absolute: true, include: "file", dot: false, symlink: true }).catch(() => [])
      for (const location of locations.toSorted()) {
        const md = await ConfigMarkdown.parse(location).catch(() => undefined)
        if (!md) continue
        const header = Info.pick({ name: true, description: true }).safeParse(md.data)
        if (!header.success) continue
        const paths = parsePaths((md.data as Record<string, unknown>).paths)
        result.push({
          ...header.data,
          location,
          content: md.content,
          source: "project",
          priority: PRIORITY.project,
          root,
          metadata: md.data as Record<string, unknown>,
          builtin: false,
          paths,
          conditional: paths !== undefined,
        })
      }
    }
    return result
  }
}
