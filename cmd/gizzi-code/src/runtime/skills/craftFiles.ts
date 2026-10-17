/**
 * Files of the built-in craft skills (bundled/craft/<skill>/: SKILL.md plus its
 * scripts, engines and templates). Kept dependency-free on purpose: the
 * production build (script/build-production.js) imports walkCraft() to inline
 * every file into the binary, and replaces readCraft() with that snapshot.
 */
import fs from "fs"
import path from "path"

export type CraftFile = { path: string; data: string; encoding: "utf8" | "base64" }
export type CraftSkillFiles = Record<string, CraftFile[]>

const TEXT = /\.(md|txt|js|mjs|cjs|ts|json|html|css|py|sh|svg|toml|yaml|yml)$|(^|\/)LICENSE$/i
const SKIP = new Set(["node_modules", "out", ".DS_Store", "__pycache__"])

export const CRAFT_DIR = path.join(import.meta.dir, "bundled", "craft")

export function walkCraft(root = CRAFT_DIR): CraftSkillFiles {
  const skills: CraftSkillFiles = {}
  if (!fs.existsSync(root)) return skills
  for (const name of fs.readdirSync(root).sort()) {
    const dir = path.join(root, name)
    if (!fs.statSync(dir).isDirectory() || !fs.existsSync(path.join(dir, "SKILL.md"))) continue
    const files: CraftFile[] = []
    const walk = (rel: string) => {
      for (const entry of fs.readdirSync(path.join(dir, rel)).sort()) {
        if (SKIP.has(entry)) continue
        const relPath = rel ? `${rel}/${entry}` : entry
        const abs = path.join(dir, relPath)
        const stat = fs.lstatSync(abs)
        if (stat.isSymbolicLink()) continue
        if (stat.isDirectory()) walk(relPath)
        else if (TEXT.test(relPath)) files.push({ path: relPath, data: fs.readFileSync(abs, "utf8"), encoding: "utf8" })
        else files.push({ path: relPath, data: fs.readFileSync(abs).toString("base64"), encoding: "base64" })
      }
    }
    walk("")
    skills[name] = files
  }
  return skills
}

/** Replaced by the inlined snapshot in production builds. */
export function readCraft(): CraftSkillFiles {
  return walkCraft()
}
