// Tailscale/Headscale mesh integration for `gizzi serve --mesh`. Joins the
// Allternit tailnet (coordination server: https://mail.news.allternit.com:8444)
// so clients on the tailnet (iOS app with embedded tsnet) can reach this
// server over WireGuard instead of a public tunnel. Mirrors tunnel.ts: one
// discovery chain, one spawn path, `stop()` lifecycle owned by this process.
//
// Three join paths, decided at start() in this order. The Allternit mesh is
// the product's network — a personal tailnet is the user's own — so the
// sidecar that joins OUR headscale is the primary path and any system
// tailscaled is only a fallback:
//   sidecar — primary path, when the mesh-node binary is discoverable AND an
//             auth key is configured: spawn the mesh-node tsnet sidecar
//             (infrastructure/mesh/tsnet-ios/cmd/mesh-node, vendored under
//             vendor/mesh-node/<platform>-<arch>/). It joins the tailnet in
//             pure userspace and listens on the tailnet, forwarding to
//             127.0.0.1:<gizzi port>, and prints `MESH_READY ip=<100.x>` which
//             we scrape. tsnet's userspace Listen accepts inbound tailnet
//             connections, so this mesh URL IS routable — no root, no TUN,
//             and it works even when gizzi itself binds loopback. It keeps
//             its own state/socket, so it coexists with any system tailscaled
//             without touching the user's personal tailnet.
//   attach  — fallback when the sidecar can't run (binary missing) or the
//             join fails (bad/expired key, control unreachable — logged with a
//             warning): a system tailscaled is reachable through the
//             `tailscale` CLI's default socket (e.g. Homebrew's `tailscaled`
//             running as a daemon, or the Tailscale.app GUI). If it already
//             has a tailnet IPv4 we reuse it as-is (passive — never `up`,
//             never `down`); otherwise we `tailscale up --login-server
//             <control>` against OUR control URL. Real host interface, so the
//             mesh URL is always routable.
//   spawn   — last resort when neither the sidecar nor a system daemon is
//             available: spawn our own tailscaled in userspace networking
//             mode (no root/TUN needed) with state/socket under the gizzi
//             data dir, then drive it via `tailscale --socket <sock>`.
//             Caveat: userspace mode gives the host no real 100.x interface,
//             so the registered mesh URL is NOT routable from other tailnet
//             nodes (no inbound forwarder on this path).
//
// Mesh is strictly additive: any join failure (missing binaries, expired or
// single-use auth key, unreachable control server) rejects start(), and the
// caller logs it and keeps serving without mesh.
import { spawn, execFile, type ChildProcess } from "node:child_process"
import { existsSync } from "node:fs"
import os from "node:os"
import { dirname, join } from "node:path"
import { Log } from "@/shared/util/log"
import { Flag } from "@/runtime/context/flag/flag"
import { Global } from "@/runtime/context/global"
import { CLOUD_URLS } from "@/shared/constants/cloudUrls"

export namespace Mesh {
  const log = Log.create({ service: "mesh" })

  export const DEFAULT_CONTROL_URL: string = CLOUD_URLS.headscale

  const INSTALL_HINT =
    "no system tailscaled is reachable, the mesh-node sidecar is not available, and the tailscale CLI is not installed. Install tailscale (`brew install tailscale`, or see https://tailscale.com/download), set GIZZI_MESH_NODE_BIN to a mesh-node binary, or set GIZZI_TAILSCALE_BIN to the CLI path."
  const DAEMON_INSTALL_HINT =
    "no system tailscaled is running, the mesh-node sidecar is not available, and the tailscaled binary is not installed, so the mesh cannot be joined. Install tailscaled (`brew install tailscale`), set GIZZI_MESH_NODE_BIN to a mesh-node binary, or set GIZZI_TAILSCALED_BIN to its path."
  const AUTH_KEY_HINT =
    "tailscale rejected the auth key — preauth keys expire and are usually single-use. Get a new one (see infrastructure/mesh/headscale/OPS.md) and pass --mesh-auth-key or set GIZZI_MESH_AUTH_KEY."
  const NO_AUTH_KEY_HINT =
    "no mesh auth key configured; skipping the mesh join. Keys are minted by pairing/enrollment — run `gizzi pair` (the platform's /api/v1/mesh/enroll flow issues the key), then pass --mesh-auth-key or set GIZZI_MESH_AUTH_KEY."

  export type Options = {
    authKey?: string
    controlUrl?: string
  }

  type Mode = "attach-passive" | "attach-up" | "sidecar" | "spawn-own"

  let child: ChildProcess | undefined
  let meshUrl: string | undefined
  let starting: Promise<string | undefined> | undefined
  let mode: Mode | undefined
  // True only when this session ran `tailscale up` with an auth key — stop()
  // must never tear down a pre-existing system tailscale login.
  let broughtUp = false

  export function url(): string | undefined {
    return meshUrl
  }

  export function active(): boolean {
    return meshUrl !== undefined
  }

  // Binary discovery follows tunnel.ts exactly: env override → vendored
  // sibling (desktop resources/bin, dist output) → npm-style vendor tree →
  // PATH. The vendor tree is vendor/<dir>/<platform>-<arch>/<name> where
  // <dir> is "tailscale" for the CLI/daemon and "mesh-node" for the sidecar.
  function findBinary(override: string | undefined, name: "tailscale" | "tailscaled" | "mesh-node"): string | undefined {
    const execDir = dirname(process.execPath)
    const platformArch = `${process.platform}-${process.arch}`
    const candidates = [
      // 1. Explicit override.
      override,
      // 2. Vendored sibling (desktop resources/bin, dist output).
      join(execDir, name),
      // 3. npm-style vendor tree.
      join(execDir, "vendor", name === "mesh-node" ? "mesh-node" : "tailscale", platformArch, name),
      // 4. PATH.
      Bun.which(name) ?? undefined,
    ].filter(Boolean) as string[]
    return candidates.find((bin) => existsSync(bin))
  }

  export function cliBinary(): string | undefined {
    return findBinary(Flag.GIZZI_TAILSCALE_BIN, "tailscale")
  }

  export function daemonBinary(): string | undefined {
    return findBinary(Flag.GIZZI_TAILSCALED_BIN, "tailscaled")
  }

  export function nodeBinary(): string | undefined {
    return findBinary(Flag.GIZZI_MESH_NODE_BIN, "mesh-node")
  }

  export function available(): boolean {
    return cliBinary() !== undefined || nodeBinary() !== undefined
  }

  function hostname(): string {
    const host = os
      .hostname()
      .toLowerCase()
      .replace(/[^a-z0-9-]+/g, "-")
      .replace(/^-+|-+$/g, "")
    return `gizzi-${host || "host"}`
  }

  // Runs the tailscale CLI and captures output; resolves with the exit code
  // instead of rejecting so callers can branch on "daemon unreachable" vs
  // "command failed". Times out so a stuck `up` (e.g. waiting on interactive
  // login) can never wedge startup.
  function runCli(
    cli: string,
    args: string[],
    timeoutMs = 60_000,
  ): Promise<{ code: number; stdout: string; stderr: string }> {
    return new Promise((resolve) => {
      execFile(cli, args, { timeout: timeoutMs }, (err, stdout, stderr) => {
        const code = err && typeof err.code === "number" ? err.code : err ? 1 : 0
        resolve({ code, stdout: stdout ?? "", stderr: stderr ?? "" })
      })
    })
  }

  async function meshIp(cli: string, socket?: string): Promise<string | undefined> {
    const args = socket ? ["--socket", socket, "ip", "-4"] : ["ip", "-4"]
    const result = await runCli(cli, args, 10_000)
    const ip = result.stdout
      .split("\n")
      .map((line) => line.trim())
      .find((line) => /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(line))
    return ip
  }

  // `tailscale status` exit 0 means the CLI could talk to a local tailscaled
  // over its default socket; a failure (exit non-zero, "failed to connect")
  // means no usable system daemon and we fall back to spawn-own.
  async function systemDaemonReachable(cli: string): Promise<boolean> {
    const result = await runCli(cli, ["status"], 10_000)
    return result.code === 0
  }

  function isAuthKeyError(output: string): boolean {
    return /auth-?key|unauthorized|invalid (?:auth|api)? ?key|401/i.test(output)
  }

  // Brings the node up on the tailnet. With an auth key this is fully
  // non-interactive; without one tailscale prints a login URL (forwarded to
  // the logs) and blocks until the user completes it, bounded by the CLI
  // timeout in runCli.
  async function tailscaleUp(cli: string, opts: Options & { socket?: string }): Promise<void> {
    const control = opts.controlUrl ?? DEFAULT_CONTROL_URL
    const args = [
      ...(opts.socket ? ["--socket", opts.socket] : []),
      "up",
      "--login-server",
      control,
      "--hostname",
      hostname(),
      ...(opts.authKey ? ["--auth-key", opts.authKey] : []),
    ]
    log.info("joining mesh tailnet", { control, authKey: !!opts.authKey, socket: opts.socket })
    const result = await runCli(cli, args)
    if (result.code === 0) {
      if (opts.authKey) broughtUp = true
      return
    }
    const output = `${result.stdout}\n${result.stderr}`
    for (const line of output.split("\n")) {
      if (line.trim()) log.debug("tailscale up", { line: line.trim() })
    }
    if (isAuthKeyError(output)) throw new Error(AUTH_KEY_HINT)
    throw new Error(`tailscale up failed (exit ${result.code}): ${output.trim().slice(0, 400)}`)
  }

  // Waits for our spawned userspace tailscaled to create its control socket.
  async function waitForSocket(sock: string, proc: ChildProcess): Promise<void> {
    const deadline = Date.now() + 10_000
    while (Date.now() < deadline) {
      if (existsSync(sock)) return
      if (proc.exitCode !== null) throw new Error(`tailscaled exited (code ${proc.exitCode}) before creating its socket`)
      await new Promise((resolve) => setTimeout(resolve, 100))
    }
    throw new Error("tailscaled did not create its control socket within 10 seconds")
  }

  // Spawns the mesh-node tsnet sidecar and resolves with the tailnet IPv4 it
  // reports. The sidecar joins the tailnet in pure userspace and forwards its
  // tailnet listener to 127.0.0.1:<port>, so unlike the userspace-tailscaled
  // path the registered mesh URL is actually routable from other tailnet
  // nodes. Readiness is the single `MESH_READY ip=<addr>` stdout line (the
  // same scrape pattern as cloudflared's URL in tunnel.ts); failures arrive
  // as `MESH_ERROR reason=...` on stderr or a non-zero exit.
  function startSidecar(bin: string, port: number, opts: Options): Promise<string> {
    return new Promise((resolve, reject) => {
      const control = opts.controlUrl ?? DEFAULT_CONTROL_URL
      const dataDir = join(Global.Path.data, "mesh-node")
      const args = [
        "--hostname",
        hostname(),
        "--control-url",
        control,
        "--data-dir",
        dataDir,
        "--forward",
        String(port),
        "--listen",
        String(port),
        ...(opts.authKey ? ["--auth-key", opts.authKey] : []),
      ]
      log.info("spawning mesh-node sidecar", { bin, control, port, authKey: !!opts.authKey })
      const proc = spawn(bin, args, { stdio: ["ignore", "pipe", "pipe"] })
      child = proc

      let settled = false
      const done = (ip: string) => {
        if (settled) return
        settled = true
        clearTimeout(timeout)
        resolve(ip)
      }
      const failReady = (err: Error) => {
        if (settled) return
        settled = true
        clearTimeout(timeout)
        reject(err)
      }
      const timeout = setTimeout(
        () => failReady(new Error("mesh-node did not report MESH_READY within 60 seconds")),
        60_000,
      )
      proc.stdout.on("data", (chunk: Buffer) => {
        const match = /^MESH_READY ip=(\S+)$/m.exec(chunk.toString())
        if (match) done(match[1])
      })
      proc.stderr.on("data", (chunk: Buffer) => {
        for (const line of chunk.toString().split("\n")) {
          if (!line.trim()) continue
          if (line.startsWith("MESH_ERROR")) {
            failReady(new Error(line.replace(/^MESH_ERROR\s+reason=/, "").trim() || "mesh-node failed"))
          } else {
            log.debug("mesh-node", { line: line.trim() })
          }
        }
      })
      proc.once("error", (err) => failReady(new Error(`failed to spawn mesh-node: ${err.message}`)))
      proc.once("exit", (code) => failReady(new Error(`mesh-node exited (code ${code}) before reporting MESH_READY`)))
    })
  }

  // Starts the mesh join and resolves with the mesh URL
  // (http://<tailnet-ip>:<port>). Idempotent: concurrent callers share the
  // in-flight start. Resolves undefined when no auth key is configured (mesh
  // skipped); rejects on any join failure — the caller logs it and keeps
  // serving without mesh.
  export async function start(port: number, opts: Options = {}): Promise<string | undefined> {
    if (meshUrl) return meshUrl
    starting ??= (async () => {
      const cli = cliBinary()

      const fail = (err: unknown) => {
        if (child) {
          try {
            child.kill("SIGTERM")
          } catch {
            // already gone
          }
          child = undefined
        }
        mode = undefined
        broughtUp = false
        meshUrl = undefined
        starting = undefined
        throw err
      }

      try {
        // No auth key, no mesh: every join path needs a preauth key to be
        // non-interactive, so without one we skip with a hint rather than
        // block on an interactive login or land on a personal tailnet.
        if (!opts.authKey) {
          log.warn(NO_AUTH_KEY_HINT)
          return undefined
        }

        // Path 1 — sidecar: the mesh-node tsnet sidecar joins AND listens on
        // the Allternit tailnet in pure userspace, forwarding to
        // 127.0.0.1:<port>, so the scraped IP yields a routable mesh URL. It
        // keeps its own state/socket, so it is the primary path even when a
        // system tailscaled exists — the user's personal tailnet stays
        // untouched.
        const node = nodeBinary()
        if (node) {
          try {
            mode = "sidecar"
            const ip = await startSidecar(node, port, opts)
            if (child) {
              child.once("exit", (code) => {
                log.warn("mesh-node exited", { code })
                child = undefined
                meshUrl = undefined
                starting = undefined
              })
            }
            meshUrl = `http://${ip}:${port}`
            log.info("mesh tailnet joined", { mode, url: meshUrl })
            return meshUrl
          } catch (err) {
            // Sidecar present but the join failed (bad/expired key, control
            // unreachable) — fall back to attaching to a system tailscaled.
            log.warn("mesh-node sidecar failed; falling back to a system tailscaled", {
              error: err instanceof Error ? err.message : String(err),
            })
            if (child) {
              try {
                child.kill("SIGTERM")
              } catch {
                // already gone
              }
              child = undefined
            }
            mode = undefined
          }
        }

        // Path 2 — attach: a system tailscaled is reachable. If it already has
        // a tailnet IPv4, reuse it passively — the machine is already on a
        // tailnet and we must not re-up or later tear down someone else's
        // login. Otherwise `tailscale up` against OUR control URL.
        if (cli && (await systemDaemonReachable(cli))) {
          const existing = await meshIp(cli)
          if (existing) {
            mode = "attach-passive"
            meshUrl = `http://${existing}:${port}`
            log.info("system tailscaled already on a tailnet; using existing mesh IP", { ip: existing })
            return meshUrl
          }
          mode = "attach-up"
          await tailscaleUp(cli, opts)
        } else {
          // Path 3 — spawn fallback: userspace networking needs no root/TUN.
          // State and socket live under the gizzi data dir so concurrent gizzi
          // instances never collide with a system daemon. Caveat (see header):
          // without the sidecar's forwarder the registered URL is unroutable.
          if (!cli) throw new Error(INSTALL_HINT)
          const daemon = daemonBinary()
          if (!daemon) throw new Error(DAEMON_INSTALL_HINT)
          const sock = join(Global.Path.data, "tailscaled.sock")
          const state = join(Global.Path.data, "tailscaled.state")
          log.info("no system tailscaled reachable; spawning userspace tailscaled", { bin: daemon, sock })
          const proc = spawn(
            daemon,
            [`--tun=userspace-networking`, `--state=${state}`, `--socket=${sock}`],
            { stdio: ["ignore", "pipe", "pipe"] },
          )
          child = proc
          const onData = (chunk: Buffer) => {
            for (const line of chunk.toString().split("\n")) {
              if (line.trim()) log.debug("tailscaled", { line: line.trim() })
            }
          }
          proc.stdout.on("data", onData)
          proc.stderr.on("data", onData)
          proc.once("exit", (code) => {
            log.warn("tailscaled exited", { code })
            child = undefined
            meshUrl = undefined
            starting = undefined
          })
          await waitForSocket(sock, proc)
          mode = "spawn-own"
          await tailscaleUp(cli, { ...opts, socket: sock })
        }

        const ip = await meshIp(cli, mode === "spawn-own" ? join(Global.Path.data, "tailscaled.sock") : undefined)
        if (!ip) throw new Error("tailscale up succeeded but no tailnet IPv4 was assigned")
        meshUrl = `http://${ip}:${port}`
        log.info("mesh tailnet joined", { mode, url: meshUrl })
        return meshUrl
      } catch (err) {
        fail(err)
      }
    })()
    return starting
  }

  // Tears down only what this session owns: `tailscale down` only when WE
  // brought the node up with an auth key (a pre-existing system login is
  // left untouched), and the spawned sidecar/tailscaled only when we spawned
  // one (SIGTERM — the sidecar closes its listener and tsnet server itself).
  export async function stop() {
    const cli = cliBinary()
    if (broughtUp && cli) {
      const sock = mode === "spawn-own" ? join(Global.Path.data, "tailscaled.sock") : undefined
      log.info("leaving mesh tailnet", { mode })
      const args = sock ? ["--socket", sock, "down"] : ["down"]
      const result = await runCli(cli, args, 10_000)
      if (result.code !== 0) log.warn("tailscale down failed", { stderr: result.stderr.trim() })
    }
    if (child) {
      log.info(mode === "sidecar" ? "stopping mesh-node sidecar" : "stopping userspace tailscaled")
      try {
        child.kill("SIGTERM")
      } catch {
        // already gone
      }
      child = undefined
    }
    mode = undefined
    broughtUp = false
    meshUrl = undefined
    starting = undefined
  }
}
