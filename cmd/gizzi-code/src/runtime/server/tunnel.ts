// cloudflared tunnel integration for `gizzi serve --tunnel`. Two modes share
// one spawn path:
//   quick — `cloudflared tunnel --url http://127.0.0.1:<port>`; the generated
//           https://*.trycloudflare.com URL is scraped from its output.
//   named — `cloudflared tunnel run --token <token>` against the user's own
//           Cloudflare account. The hostname lives on the Cloudflare side, so
//           the URL is only known when the user also supplies it.
// Binary discovery follows the same pattern as the allternit-mux auto-spawn
// (pty/index.ts): env override → vendored siblings → PATH. The child is owned
// by this process and killed on server shutdown (unlike the mux daemon, it
// must not outlive us).
import { spawn, type ChildProcess } from "node:child_process"
import { existsSync } from "node:fs"
import { dirname, join } from "node:path"
import { Log } from "@/shared/util/log"
import { Flag } from "@/runtime/context/flag/flag"

export namespace Tunnel {
  const log = Log.create({ service: "tunnel" })

  const INSTALL_HINT =
    "cloudflared is not installed. Install it (`brew install cloudflared`, or see https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) or set GIZZI_CLOUDFLARED_BIN to its path."

  export type Options = { mode: "quick" } | { mode: "named"; token: string; hostname?: string }

  let child: ChildProcess | undefined
  let publicUrl: string | undefined
  let starting: Promise<string | undefined> | undefined

  export function url(): string | undefined {
    return publicUrl
  }

  export function active(): boolean {
    return child !== undefined
  }

  export function binary(): string | undefined {
    const execDir = dirname(process.execPath)
    const platformArch = `${process.platform}-${process.arch}`
    const candidates = [
      // 1. Explicit override.
      Flag.GIZZI_CLOUDFLARED_BIN,
      // 2. Vendored sibling (desktop resources/bin, dist output).
      join(execDir, "cloudflared"),
      // 3. npm-style vendor tree.
      join(execDir, "vendor", "cloudflared", platformArch, "cloudflared"),
      // 4. PATH.
      Bun.which("cloudflared") ?? undefined,
    ].filter(Boolean) as string[]
    return candidates.find((bin) => existsSync(bin))
  }

  export function available(): boolean {
    return binary() !== undefined
  }

  // Starts the tunnel and resolves with the public URL. Idempotent: concurrent
  // callers share the in-flight start. Named tunnels without a configured
  // hostname resolve with undefined — the tunnel still runs, there is just no
  // URL to report.
  export async function start(port: number, opts: Options = { mode: "quick" }): Promise<string | undefined> {
    if (publicUrl) return publicUrl
    starting ??= (async () => {
      const bin = binary()
      if (!bin) throw new Error(INSTALL_HINT)
      // --no-autoupdate is a global cloudflared flag, accepted by both the
      // `tunnel` (quick) and `tunnel run` (named) subcommands.
      const args =
        opts.mode === "named"
          ? ["tunnel", "run", "--token", opts.token, "--no-autoupdate"]
          : ["tunnel", "--url", `http://127.0.0.1:${port}`, "--no-autoupdate"]
      log.info("starting cloudflared tunnel", { bin, mode: opts.mode })

      const proc = spawn(bin, args, {
        stdio: ["ignore", "pipe", "pipe"],
      })
      child = proc

      // In named mode the URL is known up front (or never); quick-mode URLs
      // are scraped from cloudflared's output in waitReady below.
      if (opts.mode === "named" && opts.hostname) publicUrl = `https://${opts.hostname}`

      const url = await waitReady(proc, opts).catch((err) => {
        try {
          proc.kill("SIGTERM")
        } catch {
          // already gone
        }
        child = undefined
        publicUrl = undefined
        starting = undefined
        throw err
      })

      log.info("cloudflared tunnel established", { mode: opts.mode, url })
      proc.once("exit", (code) => {
        log.warn("cloudflared exited", { code })
        child = undefined
        publicUrl = undefined
        starting = undefined
      })
      return url
    })()
    return starting
  }

  // Resolves once the tunnel is up. Quick mode waits for the scraped
  // trycloudflare URL; named mode waits for cloudflared's "Registered tunnel
  // connection" line, falling back to success on timeout — the process is
  // alive, readiness output just varies across cloudflared versions.
  function waitReady(proc: ChildProcess, opts: Options): Promise<string | undefined> {
    return new Promise((resolve, reject) => {
      let settled = false
      const done = (value?: string) => {
        if (settled) return
        settled = true
        clearTimeout(timeout)
        resolve(value)
      }
      const fail = (err: Error) => {
        if (settled) return
        settled = true
        clearTimeout(timeout)
        reject(err)
      }
      const timeout = setTimeout(() => {
        if (opts.mode === "named") {
          log.warn("cloudflared did not confirm the named tunnel within 30 seconds; assuming it is running")
          done(publicUrl)
        } else {
          fail(new Error("cloudflared did not print a tunnel URL within 30 seconds"))
        }
      }, 30_000)
      const onData = (chunk: Buffer) => {
        const text = chunk.toString()
        if (opts.mode === "quick") {
          const match = /https:\/\/[a-z0-9-]+\.trycloudflare\.com/.exec(text)
          if (match) {
            publicUrl = match[0]
            done(match[0])
          }
        } else if (/Registered tunnel connection/i.test(text)) {
          done(publicUrl)
        }
        // cloudflared writes its banner/URL to stderr; forward for debugging.
        for (const line of text.split("\n")) {
          if (line.trim()) log.debug("cloudflared", { line: line.trim() })
        }
      }
      proc.stdout.on("data", onData)
      proc.stderr.on("data", onData)
      proc.once("error", (err) => fail(new Error(`failed to spawn cloudflared: ${err.message}`)))
      proc.once("exit", (code) => {
        fail(new Error(`cloudflared exited (code ${code}) before the tunnel was ready`))
      })
    })
  }

  export function stop() {
    if (child) {
      log.info("stopping cloudflared tunnel")
      try {
        child.kill("SIGTERM")
      } catch {
        // already gone
      }
      child = undefined
    }
    publicUrl = undefined
    starting = undefined
  }
}
