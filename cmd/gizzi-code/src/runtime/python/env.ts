/**
 * Python environment for `run_code`: a uv-managed venv in the gizzi data dir.
 *
 * First use creates `<data>/python/venv` with CPython 3.12 (uv fetches its own
 * managed CPython into `<data>/python/cpython`, so the host needs no Python)
 * and installs the pinned analysis packages. That one-time setup is the only
 * step that touches the network; scripts themselves always run network-off in
 * the OS sandbox. Setup is serialized across concurrent calls (in-process
 * promise) and across gizzi processes sharing the data dir (a lock dir).
 *
 * uv is never bundled. Resolution order matches Desktop's
 * `system-one-manager.ts resolveUv`: `UV`, PATH, ~/.local/bin, ~/.cargo/bin,
 * /opt/homebrew/bin, /usr/local/bin, then a uv this module downloaded before.
 * With none of those, it downloads the pinned uv release below, checks its
 * sha256, and installs it under `<data>/python/uv/<version>/` — and says so in
 * the setup notes the tool result carries. Never silent.
 */
import path from "path"
import os from "os"
import fs from "fs/promises"
import fsSync from "fs"
import { spawn } from "child_process"
import { createHash } from "crypto"
import { Global } from "@/runtime/context/global"
import { Log } from "@/shared/util/log"
import LOCK_TEXT from "./requirements.lock.txt"

export namespace PythonEnv {
  const log = Log.create({ service: "python-env" })

  export const PYTHON_VERSION = "3.12"
  export const PACKAGES = ["pandas==2.3.3", "numpy==2.3.5", "openpyxl==3.1.5", "matplotlib==3.10.7"] as const

  /**
   * Hash-pinned lock of PACKAGES and every transitive dependency (universal:
   * macOS/Linux, arm64/x64). Bundled as text; written to the data dir at
   * setup and installed with --require-hashes --only-binary :all:.
   */
  export const LOCK: string = LOCK_TEXT
  export const LOCK_HASH: string = createHash("sha256").update(LOCK_TEXT).digest("hex").slice(0, 16)

  /** Minimal env for uv: no API keys or tokens from the parent process. */
  const ENV_PASSTHROUGH = [
    "PATH", "HOME", "USERPROFILE", "TMPDIR", "TEMP", "TMP", "LANG", "LC_ALL",
    "UV_CACHE_DIR", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME",
    "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "ALL_PROXY",
    "https_proxy", "http_proxy", "no_proxy", "all_proxy",
    "SSL_CERT_FILE", "SSL_CERT_DIR", "REQUESTS_CA_BUNDLE",
  ]
  export function uvEnv(src: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
    const env: NodeJS.ProcessEnv = {}
    for (const k of ENV_PASSTHROUGH) if (src[k] !== undefined) env[k] = src[k]
    env.UV_PYTHON_INSTALL_DIR = path.join(root(), "cpython")
    env.UV_NO_PROGRESS = "1"
    return env
  }

  /** The uv release downloaded when the host has none, with its sha256 per target. */
  export const UV_VERSION = "0.11.19"
  export const UV_SHA256: Record<string, string> = {
    "aarch64-apple-darwin": "d8f59c38e8c4168ee468d423cd63184be12fa6995a4283d41ee1a14d003c9453",
    "x86_64-apple-darwin": "1585f415cade9f061e7f00fe5b00030a79ccfac60c650242ce639ba946138d40",
    "x86_64-unknown-linux-gnu": "7035608168e106375b36d0c818d537a889c51a8625fe7f8f7cad5e62b947c368",
    "aarch64-unknown-linux-gnu": "83b13ab184a45b7d9a3b0e4b10eaebd50ad41e66cb16dcce8e60aa7be13ae399",
  }

  const INSTALL_TIMEOUT_MS = 15 * 60_000
  const LOCK_STALE_MS = 20 * 60_000
  const MARKER = ".allternit-env.json"

  /** Root of everything this module owns. `GIZZI_PYTHON_ENV_DIR` overrides it (tests). */
  export function root(): string {
    return process.env.GIZZI_PYTHON_ENV_DIR || path.join(Global.Path.data, "python")
  }
  export function venvDir(): string {
    return path.join(root(), "venv")
  }
  export function pythonBin(): string {
    return process.platform === "win32"
      ? path.join(venvDir(), "Scripts", "python.exe")
      : path.join(venvDir(), "bin", "python")
  }

  /** The uv release target for this host, or undefined when none is pinned. */
  export function uvTarget(platform = process.platform, arch = process.arch): string | undefined {
    const cpu = arch === "arm64" ? "aarch64" : arch === "x64" ? "x86_64" : undefined
    if (!cpu) return undefined
    if (platform === "darwin") return `${cpu}-apple-darwin`
    if (platform === "linux") return `${cpu}-unknown-linux-gnu`
    return undefined
  }

  function downloadedUv(): string {
    return path.join(root(), "uv", UV_VERSION, process.platform === "win32" ? "uv.exe" : "uv")
  }

  /** uv from UV, PATH, the usual install locations, or a pinned copy fetched earlier. */
  export function resolveUv(env: NodeJS.ProcessEnv = process.env, home = os.homedir()): string | undefined {
    const exe = process.platform === "win32" ? "uv.exe" : "uv"
    const candidates = [
      env.UV,
      ...(env.PATH ?? "").split(path.delimiter).filter(Boolean).map((d) => path.join(d, exe)),
      path.join(home, ".local", "bin", exe),
      path.join(home, ".cargo", "bin", exe),
      "/opt/homebrew/bin/uv",
      "/usr/local/bin/uv",
      downloadedUv(),
    ]
    return candidates.find((c): c is string => !!c && fsSync.existsSync(c))
  }

  /**
   * Download the pinned uv release for this host, verify its sha256, and
   * install it at `<root>/uv/<version>/uv`. Throws on a checksum mismatch.
   */
  export async function downloadUv(opts: { signal?: AbortSignal; fetchImpl?: typeof fetch } = {}): Promise<string> {
    const target = uvTarget()
    if (!target) {
      throw new Error(
        `uv is not installed and there is no pinned uv build for ${process.platform}/${process.arch}. ` +
          "Install uv (https://docs.astral.sh/uv/) and try again.",
      )
    }
    const expected = UV_SHA256[target]
    const url = `https://github.com/astral-sh/uv/releases/download/${UV_VERSION}/uv-${target}.tar.gz`
    const res = await (opts.fetchImpl ?? fetch)(url, { signal: opts.signal, redirect: "follow" })
    if (!res.ok) throw new Error(`Downloading uv ${UV_VERSION} failed: HTTP ${res.status} from ${url}`)
    const bytes = Buffer.from(await res.arrayBuffer())
    const actual = createHash("sha256").update(bytes).digest("hex")
    if (actual !== expected) {
      throw new Error(`uv ${UV_VERSION} download failed its checksum (expected ${expected}, got ${actual}); not installed.`)
    }
    const dest = path.dirname(downloadedUv())
    const staging = `${dest}.tmp-${process.pid}-${Date.now()}`
    await fs.mkdir(staging, { recursive: true })
    try {
      const tarball = path.join(staging, "uv.tar.gz")
      await fs.writeFile(tarball, bytes)
      await run("tar", ["-xzf", tarball, "-C", staging], { signal: opts.signal, timeoutMs: 60_000 })
      const extracted = path.join(staging, `uv-${target}`, "uv")
      await fs.chmod(extracted, 0o755)
      await fs.rm(dest, { recursive: true, force: true })
      await fs.mkdir(dest, { recursive: true })
      await fs.rename(extracted, downloadedUv())
    } finally {
      await fs.rm(staging, { recursive: true, force: true })
    }
    return downloadedUv()
  }

  interface Marker {
    python: string
    packages: string[]
    lock?: string
  }

  /** True when the venv exists and was built with the current pins. */
  export async function isReady(): Promise<boolean> {
    if (!fsSync.existsSync(pythonBin())) return false
    try {
      const marker = JSON.parse(await fs.readFile(path.join(venvDir(), MARKER), "utf8")) as Marker
      return marker.python === PYTHON_VERSION && JSON.stringify(marker.packages) === JSON.stringify(PACKAGES) && marker.lock === LOCK_HASH
    } catch {
      return false
    }
  }

  export interface EnsureResult {
    /** The venv's python. */
    python: string
    /** Plain-language notes on one-time setup done during this call (empty when already set up). */
    setup: string[]
  }

  let inflight: Promise<EnsureResult> | undefined

  /**
   * The venv's python, creating the environment first if needed. `onStatus`
   * receives short progress lines for the UI while setup runs.
   */
  export async function ensure(opts: { signal?: AbortSignal; onStatus?: (line: string) => void } = {}): Promise<EnsureResult> {
    if (await isReady()) return { python: pythonBin(), setup: [] }
    if (!inflight) {
      inflight = withLock(() => install(opts)).finally(() => {
        inflight = undefined
      })
    }
    return inflight
  }

  async function install(opts: { signal?: AbortSignal; onStatus?: (line: string) => void }): Promise<EnsureResult> {
    // Another process may have finished while this one waited for the lock.
    if (await isReady()) return { python: pythonBin(), setup: [] }
    const setup: string[] = []
    let uv = resolveUv()
    if (!uv) {
      opts.onStatus?.(`Downloading uv ${UV_VERSION} (one-time)`)
      uv = await downloadUv({ signal: opts.signal })
      setup.push(`uv was not installed, so uv ${UV_VERSION} was downloaded (sha256 verified) to ${uv}. One-time.`)
    }
    // A dedicated install dir keeps the managed CPython next to the venv, so
    // the sandbox can bind one read-only tree on Linux.
    const env = uvEnv()
    await fs.rm(venvDir(), { recursive: true, force: true })
    await fs.mkdir(root(), { recursive: true })
    opts.onStatus?.(`Setting up Python ${PYTHON_VERSION} (one-time)`)
    await run(
      uv,
      ["venv", venvDir(), "--python", PYTHON_VERSION, "--python-preference", "only-managed", "--no-config"],
      { env, signal: opts.signal, timeoutMs: INSTALL_TIMEOUT_MS },
    )
    opts.onStatus?.(`Installing ${PACKAGES.join(", ")} (one-time)`)
    const lockFile = path.join(root(), `requirements-${LOCK_HASH}.lock`)
    await fs.writeFile(lockFile, LOCK)
    await run(
      uv,
      ["pip", "install", "--python", pythonBin(), "--no-config", "--require-hashes", "--only-binary", ":all:", "-r", lockFile],
      {
        env,
        signal: opts.signal,
        timeoutMs: INSTALL_TIMEOUT_MS,
      },
    )
    const marker: Marker = { python: PYTHON_VERSION, packages: [...PACKAGES], lock: LOCK_HASH }
    await fs.writeFile(path.join(venvDir(), MARKER), JSON.stringify(marker))
    setup.push(
      `Set up a Python ${PYTHON_VERSION} environment with ${PACKAGES.join(", ")} in ${venvDir()}. One-time; later runs reuse it.`,
    )
    log.info("python env ready", { venv: venvDir() })
    return { python: pythonBin(), setup }
  }

  /** Cross-process lock: a directory under root, broken when stale. */
  async function withLock<T>(fn: () => Promise<T>): Promise<T> {
    await fs.mkdir(root(), { recursive: true })
    const lock = path.join(root(), ".setup.lock")
    const deadline = Date.now() + INSTALL_TIMEOUT_MS + 60_000
    for (;;) {
      try {
        await fs.mkdir(lock)
        break
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error
        const stat = await fs.stat(lock).catch(() => undefined)
        if (stat && Date.now() - stat.mtimeMs > LOCK_STALE_MS) {
          await fs.rm(lock, { recursive: true, force: true })
          continue
        }
        if (Date.now() > deadline) throw new Error("Timed out waiting for another Python setup to finish")
        await new Promise((r) => setTimeout(r, 500))
      }
    }
    try {
      return await fn()
    } finally {
      await fs.rm(lock, { recursive: true, force: true })
    }
  }

  /** Run a setup command; rejects with its stderr tail on a non-zero exit. */
  function run(
    bin: string,
    args: string[],
    opts: { env?: NodeJS.ProcessEnv; signal?: AbortSignal; timeoutMs: number },
  ): Promise<void> {
    return new Promise((resolve, reject) => {
      const proc = spawn(bin, args, { env: opts.env ?? process.env, stdio: ["ignore", "pipe", "pipe"] })
      let stderr = ""
      proc.stdout?.on("data", () => {})
      proc.stderr?.on("data", (chunk: Buffer) => {
        stderr = (stderr + chunk.toString()).slice(-4000)
      })
      const timer = setTimeout(() => proc.kill("SIGKILL"), opts.timeoutMs)
      const onAbort = () => proc.kill("SIGKILL")
      opts.signal?.addEventListener("abort", onAbort, { once: true })
      proc.once("error", (error) => {
        clearTimeout(timer)
        opts.signal?.removeEventListener("abort", onAbort)
        reject(error)
      })
      proc.once("close", (code, signal) => {
        clearTimeout(timer)
        opts.signal?.removeEventListener("abort", onAbort)
        if (code === 0) return resolve()
        reject(new Error(`${path.basename(bin)} ${args[0]} failed (${signal ?? `exit ${code}`}): ${stderr.trim()}`))
      })
    })
  }
}
