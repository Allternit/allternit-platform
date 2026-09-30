import { mkdir, writeFile, readFile, symlink, lstat } from "node:fs/promises"
import { dirname, resolve, join } from "node:path"
import { fileURLToPath } from "node:url"
import type { Check, Files, Fixture, Verification } from "./types"

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../..")
export const DEPENDENCIES = join(ROOT, "node_modules")
export async function command(argv: string[], cwd: string, timeoutMs = 30000): Promise<Check> {
  const proc = Bun.spawn(argv, { cwd, stdout: "pipe", stderr: "pipe", env: { ...process.env, CI: "1", NO_COLOR: "1" } })
  let timedOut = false
  const timer = setTimeout(() => { timedOut = true; proc.kill() }, timeoutMs)
  try {
    const [exitCode, stdout, stderr] = await Promise.all([proc.exited, new Response(proc.stdout).text(), new Response(proc.stderr).text()])
    return { passed: exitCode === 0 && !timedOut, exitCode, output: (timedOut ? "Timeout\n" : "") + (stdout + stderr).slice(-12000) }
  } finally { clearTimeout(timer) }
}
export async function writeFiles(repo: string, files: Files) {
  for (const [path, content] of Object.entries(files)) {
    const full = resolve(repo, path)
    if (!full.startsWith(resolve(repo) + "/")) throw new Error("file escapes workspace")
    await mkdir(dirname(full), { recursive: true })
    await writeFile(full, content)
  }
}
export async function materialize(fixture: Fixture, repo: string) {
  await mkdir(repo, { recursive: true })
  await writeFiles(repo, fixture.files)
  await symlink(DEPENDENCIES, join(repo, "node_modules"), "dir")
  for (const args of [["git", "init", "--quiet"], ["git", "add", "src", "tests", "package.json", "vitest.config.ts"]]) {
    const result = await command(args, repo)
    if (!result.passed) throw new Error(result.output)
  }
}
export async function sourceFiles(repo: string): Promise<Files> {
  const files: Files = {}
  for (const path of ["src/lib.ts", "src/dep.ts"]) {
    if (!(await lstat(join(repo, path))).isFile()) throw new Error("source must be a regular file")
    files[path] = await readFile(join(repo, path), "utf8")
  }
  return files
}
/** Only source mutations are allowed; evaluation/config never become patch targets. */
export async function applyPatch(repo: string, patch: string): Promise<boolean> {
  if (!patch || patch.length > 100000 || patch.includes("GIT binary patch")) return false
  const headers = [...patch.matchAll(/^diff --git a\/(\S+) b\/(\S+)$/gm)]
  if (headers.length !== 1 || headers[0][1] !== "src/lib.ts" || headers[0][2] !== "src/lib.ts") return false
  if (!/^--- a\/src\/lib\.ts$/m.test(patch) || !/^\+\+\+ b\/src\/lib\.ts$/m.test(patch)) return false
  if (/^(?:new file mode|deleted file mode|old mode|new mode|rename|copy|--- |\+\+\+ )/m.test(
    patch.replace(/^--- a\/src\/lib\.ts\n/m, "").replace(/^\+\+\+ b\/src\/lib\.ts\n/m, ""))) return false
  if (!(await lstat(join(repo, "src/lib.ts"))).isFile()) return false
  const patchPath = join(repo, ".candidate.patch")
  await writeFile(patchPath, patch)
  if (!(await command(["git", "apply", "--check", patchPath], repo)).passed) return false
  return (await command(["git", "apply", patchPath], repo)).passed
}
export async function runTests(repo: string, timeoutMs = 30000): Promise<Verification> {
  const cli = join(DEPENDENCIES, "vitest/vitest.mjs")
  const target = await command(["node", cli, "run", "tests/bug.test.ts", "--config", "vitest.config.ts"], repo, timeoutMs)
  const regression = await command(["node", cli, "run", "tests/regression.test.ts", "--config", "vitest.config.ts"], repo, timeoutMs)
  return { target, regression, integrity: true }
}
/** Fresh trusted evaluation prevents graph modifications to tests/config from gaming scores. */
export async function evaluate(fixture: Fixture, candidateRepo: string, evaluationRepo: string): Promise<Verification> {
  await materialize(fixture, evaluationRepo)
  let integrity = true
  for (const [path, content] of Object.entries(fixture.files)) {
    if (path === fixture.truth.file) continue
    try { if (!(await lstat(join(candidateRepo, path))).isFile() || await readFile(join(candidateRepo, path), "utf8") !== content) integrity = false }
    catch { integrity = false }
  }
  try { await writeFile(join(evaluationRepo, fixture.truth.file), (await sourceFiles(candidateRepo))[fixture.truth.file]) }
  catch { integrity = false }
  return { ...await runTests(evaluationRepo), integrity }
}
