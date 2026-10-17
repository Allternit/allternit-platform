/**
 * Built-in craft skills (motion graphics, brush films, UI motion, visual design,
 * generative art). Unlike the text-only BUNDLED_SKILLS these carry scripts and
 * templates, so each one is extracted to the user's data dir and then loaded
 * like any skill on disk. User and project skills of the same name still win.
 *
 * Extraction is keyed by a hash of the files, so a new gizzi version with
 * changed files lands in a fresh directory and projects that link to an older
 * extraction keep working. The directory is private to the user (0700) rather
 * than in a shared temp dir, and is written to a temp sibling then renamed.
 */
import fs from "fs/promises"
import { existsSync } from "fs"
import path from "path"
import { createHash } from "crypto"
import { Global } from "@/runtime/context/global"
import { Log } from "@/shared/util/log"
import { readCraft, type CraftFile, type CraftSkillFiles } from "./craftFiles"

const log = Log.create({ service: "skill.craft" })

/** `${SKILL_DIR}` in a skill's Markdown becomes the extracted directory. */
export const SKILL_DIR_TOKEN = "${SKILL_DIR}"

export function craftRoot(): string {
  return path.join(Global.Path.data, "skills-builtin")
}

export function craftHash(files: CraftFile[]): string {
  const hash = createHash("sha256")
  for (const file of files) hash.update(file.path).update("\0").update(file.data).update("\0")
  return hash.digest("hex").slice(0, 12)
}

async function extract(name: string, files: CraftFile[], root: string): Promise<string> {
  const dir = path.join(root, name, craftHash(files))
  if (existsSync(path.join(dir, ".complete"))) return dir
  const tmp = `${dir}.tmp-${process.pid}-${Date.now()}`
  await fs.mkdir(tmp, { recursive: true, mode: 0o700 })
  for (const file of files) {
    const target = path.join(tmp, file.path)
    if (!target.startsWith(tmp + path.sep)) throw new Error(`craft skill ${name}: bad path ${file.path}`)
    await fs.mkdir(path.dirname(target), { recursive: true })
    const data =
      file.encoding === "base64"
        ? Buffer.from(file.data, "base64")
        : file.path.endsWith(".md")
          ? file.data.split(SKILL_DIR_TOKEN).join(dir)
          : file.data
    await fs.writeFile(target, data)
    if (/\.(sh|py|mjs)$/.test(file.path)) await fs.chmod(target, 0o755)
  }
  await fs.writeFile(path.join(tmp, ".complete"), "")
  // A leftover from an interrupted extraction (no .complete) is replaced.
  if (existsSync(dir) && !existsSync(path.join(dir, ".complete"))) await fs.rm(dir, { recursive: true, force: true })
  await fs.rename(tmp, dir).catch(async (err) => {
    // Another process extracted the same version first: keep theirs.
    await fs.rm(tmp, { recursive: true, force: true })
    if (!existsSync(path.join(dir, ".complete"))) throw err
  })
  return dir
}

/** Extract every craft skill (no-op when already extracted); returns their SKILL.md paths. */
export async function materializeCraftSkills(root = craftRoot(), skills: CraftSkillFiles = readCraft()): Promise<string[]> {
  const out: string[] = []
  for (const [name, files] of Object.entries(skills)) {
    try {
      out.push(path.join(await extract(name, files, root), "SKILL.md"))
    } catch (err) {
      log.error("failed to extract built-in skill", { name, err })
    }
  }
  return out
}
