#!/usr/bin/env bun
/**
 * MCP SDK v2 vendoring / packaging check.
 *
 * gizzi-code uses the official TypeScript SDK v2 split packages
 * (`@modelcontextprotocol/client`, `/server`, `/core`; protocol 2026-07-28 with the
 * 2025-era fallback). The production build (`script/build-production.js`) bundles them
 * from node_modules into the compiled binary, so a release needs no vendored copy. This
 * script exists for the two jobs the v1-era vendoring script did:
 *
 *   bun script/vendor-mcp-sdk.ts            # check (default): pinned versions installed,
 *                                           # every entry gizzi imports resolves, no v1 imports
 *   bun script/vendor-mcp-sdk.ts --vendor   # also copy the three packages (package.json +
 *                                           # dist) to vendor/@modelcontextprotocol/ for an
 *                                           # offline/air-gapped build
 *
 * The v1 script rewrote source imports to relative vendored paths; v2 packages are
 * single-entry ESM with `exports`, so imports stay `@modelcontextprotocol/<pkg>` and a
 * vendored copy is used via the package manager (e.g. `file:` overrides), never by
 * rewriting source.
 */

import { cp, mkdir, readFile, rm } from "fs/promises"
import { dirname, join, resolve } from "path"
import { $ } from "bun"

const ROOT = resolve(import.meta.dir, "..")
const PACKAGES = ["client", "server", "core"] as const
/** Every entry point gizzi's source imports. */
const ENTRIES = [
  "@modelcontextprotocol/client",
  "@modelcontextprotocol/client/stdio",
  "@modelcontextprotocol/server",
  "@modelcontextprotocol/server/stdio",
  "@modelcontextprotocol/core",
]

/** Package dir of an installed v2 package (its `exports` don't expose package.json). */
async function packageDir(name: string): Promise<string> {
  let dir = dirname(Bun.resolveSync(`@modelcontextprotocol/${name}`, ROOT))
  while (dir !== dirname(dir)) {
    const pkg = await readFile(join(dir, "package.json"), "utf8").catch(() => undefined)
    if (pkg && JSON.parse(pkg).name === `@modelcontextprotocol/${name}`) return dir
    dir = dirname(dir)
  }
  throw new Error(`cannot locate @modelcontextprotocol/${name}`)
}

async function pinnedVersions(): Promise<Record<string, string>> {
  const pkg = JSON.parse(await readFile(join(ROOT, "package.json"), "utf8"))
  const deps = { ...pkg.dependencies, ...pkg.devDependencies } as Record<string, string>
  if (deps["@modelcontextprotocol/sdk"]) {
    throw new Error("package.json still depends on the v1 @modelcontextprotocol/sdk; gizzi uses the v2 packages")
  }
  const out: Record<string, string> = {}
  for (const name of PACKAGES) {
    const spec = deps[`@modelcontextprotocol/${name}`]
    if (!spec) throw new Error(`package.json is missing @modelcontextprotocol/${name}`)
    out[name] = spec
  }
  return out
}

async function check() {
  const pinned = await pinnedVersions()
  for (const name of PACKAGES) {
    const installed = JSON.parse(await readFile(join(await packageDir(name), "package.json"), "utf8")).version as string
    if (installed !== pinned[name]) {
      throw new Error(`@modelcontextprotocol/${name}: package.json pins ${pinned[name]}, installed ${installed}`)
    }
    console.log(`  ✓ @modelcontextprotocol/${name}@${installed}`)
  }
  for (const entry of ENTRIES) {
    Bun.resolveSync(entry, ROOT)
    console.log(`  ✓ resolves ${entry}`)
  }
  const v1 = await $`grep -rlE ${"--include=*.ts"} ${"--include=*.tsx"} ${"from ['\"]@modelcontextprotocol/sdk"} src test`
    .cwd(ROOT)
    .nothrow()
    .quiet()
  const offenders = v1.stdout.toString().trim()
  if (offenders) throw new Error(`v1 SDK imports remain:\n${offenders}`)
  console.log("  ✓ no @modelcontextprotocol/sdk (v1) imports in src/ or test/")
}

async function vendor() {
  const dest = join(ROOT, "vendor/@modelcontextprotocol")
  for (const name of PACKAGES) {
    const pkgDir = await packageDir(name)
    const out = join(dest, name)
    await rm(out, { recursive: true, force: true })
    await mkdir(out, { recursive: true })
    await cp(join(pkgDir, "package.json"), join(out, "package.json"))
    await cp(join(pkgDir, "dist"), join(out, "dist"), { recursive: true })
    for (const extra of ["LICENSE", "README.md"]) {
      await cp(join(pkgDir, extra), join(out, extra)).catch(() => {})
    }
    console.log(`  ✓ vendored @modelcontextprotocol/${name} → ${out}`)
  }
}

async function main() {
  console.log("MCP SDK v2 check")
  await check()
  if (process.argv.includes("--vendor")) await vendor()
}

main().catch((err) => {
  console.error(`✗ ${err instanceof Error ? err.message : String(err)}`)
  process.exit(1)
})
