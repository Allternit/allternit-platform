import { BusEvent } from "@/shared/bus/bus-event"
import path from "path"
import { $ } from "bun"
import z from "zod/v4"
import { NamedError } from "@allternit/gizzi-util/error.js"
import { Log } from "@/shared/util/log"
import { iife } from "@/shared/util/iife"
import { Flag } from "@/runtime/context/flag/flag"

declare global {
  const GIZZI_VERSION: string
  const GIZZI_CHANNEL: string
}

export namespace Installation {
  const log = Log.create({ service: "installation" })

  export type Method = Awaited<ReturnType<typeof method>>

  export const Event = {
    Updated: BusEvent.define(
      "installation.updated",
      z.object({
        version: z.string(),
      }),
    ),
    UpdateAvailable: BusEvent.define(
      "installation.update-available",
      z.object({
        version: z.string(),
      }),
    ),
  }

  export const Info = z
    .object({
      version: z.string(),
      latest: z.string(),
    })
    
  export type Info = z.infer<typeof Info>

  export async function info() {
    return {
      version: VERSION,
      latest: await latest(),
    }
  }

  export function isPreview() {
    return CHANNEL !== "latest"
  }

  export function isLocal() {
    return CHANNEL === "local"
  }

  export async function method() {
    if (process.execPath.includes(path.join(".gizzi", "bin"))) return "curl"
    if (process.execPath.includes(path.join(".local", "bin"))) return "curl"
    const exec = process.execPath.toLowerCase()

    const checks = [
      {
        name: "npm" as const,
        command: () => $`npm list -g --depth=0`.throws(false).quiet().text(),
      },
      {
        name: "yarn" as const,
        command: () => $`yarn global list`.throws(false).quiet().text(),
      },
      {
        name: "pnpm" as const,
        command: () => $`pnpm list -g --depth=0`.throws(false).quiet().text(),
      },
      {
        name: "bun" as const,
        command: () => $`bun pm ls -g`.throws(false).quiet().text(),
      },
      {
        name: "brew" as const,
        command: () => $`brew list --formula gizzi`.throws(false).quiet().text(),
      },
      {
        name: "scoop" as const,
        command: () => $`scoop list gizzi`.throws(false).quiet().text(),
      },
      {
        name: "choco" as const,
        command: () => $`choco list --limit-output gizzi`.throws(false).quiet().text(),
      },
    ]

    checks.sort((a, b) => {
      const aMatches = exec.includes(a.name)
      const bMatches = exec.includes(b.name)
      if (aMatches && !bMatches) return -1
      if (!aMatches && bMatches) return 1
      return 0
    })

    for (const check of checks) {
      const output = await check.command()
      const installedName =
        check.name === "brew" || check.name === "choco" || check.name === "scoop" ? "gizzi" : "gizzi-code"
      const legacyName =
        check.name === "brew" || check.name === "choco" || check.name === "scoop" ? "gizzi" : "gizzi-ai"
      if (output.includes(installedName) || output.includes(legacyName)) {
        return check.name
      }
    }

    return "unknown"
  }

  export const UpgradeFailedError = NamedError.create(
    "UpgradeFailedError",
    z.object({
      stderr: z.string(),
    }),
  )

  async function getBrewFormula() {
    const formula = await $`brew list --formula gizzi-code`.throws(false).quiet().text()
    if (formula.includes("gizzi-code")) return "gizzi-code"
    // Legacy formula name
    const legacy = await $`brew list --formula gizzi`.throws(false).quiet().text()
    if (legacy.includes("gizzi")) return "gizzi"
    return "gizzi-code"
  }

  export async function upgrade(method: Method, target: string) {
    let cmd
    switch (method) {
      case "curl":
        cmd = $`curl -fsSL https://install.gizziio.com/install | bash`.env({
          ...process.env,
          GIZZI_VERSION: target,
          VERSION: target,
        })
        break
      case "npm":
        cmd = $`npm install -g @allternit/gizzi-code@${target}`
        break
      case "pnpm":
        cmd = $`pnpm install -g @allternit/gizzi-code@${target}`
        break
      case "bun":
        cmd = $`bun install -g @allternit/gizzi-code@${target}`
        break
      case "brew": {
        const formula = await getBrewFormula()
        cmd = $`brew upgrade ${formula}`.env({
          HOMEBREW_NO_AUTO_UPDATE: "1",
          ...process.env,
        })
        break
      }
      case "choco":
        cmd = $`echo Y | choco upgrade gizzi --version=${target}`
        break
      case "scoop":
        cmd = $`scoop install gizzi@${target}`
        break
      default:
        throw new Error(`Unknown method: ${method}`)
    }
    const result = await cmd.quiet().throws(false)
    if (result.exitCode !== 0) {
      const stderr = method === "choco" ? "not running from an elevated command shell" : result.stderr.toString("utf8")
      throw new UpgradeFailedError({
        stderr: stderr,
      })
    }
    log.info("upgraded", {
      method,
      target,
      stdout: result.stdout.toString(),
      stderr: result.stderr.toString(),
    })
    await $`${process.execPath} --version`.nothrow().quiet().text()
  }

  export const VERSION = iife(() => {
    try {
      return GIZZI_VERSION
    } catch {
      return "local"
    }
  })
  export const CHANNEL = iife(() => {
    try {
      return GIZZI_CHANNEL
    } catch {
      return "local"
    }
  })
  export const USER_AGENT = `gizzi-code/${CHANNEL}/${VERSION}/${Flag.GIZZI_CLIENT}`

  export async function latest(installMethod?: Method) {
    const detectedMethod = installMethod || (await method())

    if (detectedMethod === "brew") {
      const formula = await getBrewFormula()
      if (formula.includes("/")) {
        const infoJson = await $`brew info --json=v2 ${formula}`.quiet().text()
        const info = JSON.parse(infoJson)
        const version = info.formulae?.[0]?.versions?.stable
        if (!version) throw new Error(`Could not detect version for tap formula: ${formula}`)
        return version
      }
      return fetch("https://formulae.brew.sh/api/formula/gizzi.json")
        .then((res) => {
          if (!res.ok) return fetch("https://formulae.brew.sh/api/formula/gizzi.json")
          return res
        })
        .then((res) => {
          if (!res.ok) throw new Error(res.statusText)
          return res.json()
        })
        .then((data: any) => data.versions.stable)
    }

    if (detectedMethod === "npm" || detectedMethod === "bun" || detectedMethod === "pnpm") {
      const registry = await iife(async () => {
        const r = (await $`npm config get registry`.quiet().nothrow().text()).trim()
        const reg = r || "https://registry.npmjs.org"
        return reg.endsWith("/") ? reg.slice(0, -1) : reg
      })
      return fetch(`${registry}/@allternit/gizzi-code/latest`)
        .then((res) => {
          if (!res.ok) throw new Error(res.statusText)
          return res.json()
        })
        .then((data: any) => data.version)
    }

    if (detectedMethod === "choco") {
      return fetch(
        "https://community.chocolatey.org/api/v2/Packages?$filter=Id%20eq%20%27gizzi%27%20and%20IsLatestVersion&$select=Version",
        { headers: { Accept: "application/json;odata=verbose" } },
      )
        .then((res) => {
          if (!res.ok) throw new Error(res.statusText)
          return res.json()
        })
        .then((data: any) => data.d.results[0]?.Version)
    }

    if (detectedMethod === "scoop") {
      return fetch("https://raw.githubusercontent.com/ScoopInstaller/Main/master/bucket/gizzi.json", {
        headers: { Accept: "application/json" },
      })
        .then((res) => {
          if (!res.ok) return fetch("https://raw.githubusercontent.com/ScoopInstaller/Main/master/bucket/gizzi.json")
          return res
        })
        .then((res) => {
          if (!res.ok) throw new Error(res.statusText)
          return res.json()
        })
        .then((data: any) => data.version)
    }

    return fetch("https://api.github.com/repos/Allternit/allternit-platform/releases/latest")
      .then((res) => {
        if (!res.ok) throw new Error(res.statusText)
        return res.json()
      })
      .then((data: any) => {
        // Tags look like "gizzi-code/0.2.3" (older ones may be "v0.2.3") —
        // reduce to the bare version in either case.
        const tag: string = data.tag_name || ""
        return tag.split("/").pop()!.replace(/^v/, "")
      })
  }
}
