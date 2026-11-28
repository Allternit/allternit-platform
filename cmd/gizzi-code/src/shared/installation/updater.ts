/**
 * Gizzi updater core: reads the signed release feed (feed.ts) and acts per
 * install method (PLAN.md D2):
 *
 *   curl / native   download the platform artifact, check its sha256 against
 *                   the signed feed, stage it next to the installed binary,
 *                   swap on exit (or on the next launch if exit never ran).
 *                   Windows renames the running exe aside before the swap.
 *   npm             run `npm i -g @allternit/gizzi-code@<v>` after exit.
 *   pnpm/bun/yarn,
 *   brew/scoop/choco print the command; never self-replace.
 *   desktop         "managed by Allternit Desktop"; no network, no writes.
 *
 * Feed `paused` = do nothing (and drop a staged update). Feed `minVersion`
 * above the running version = required: applied even when auto-update is off
 * (for the auto methods), and reported as required everywhere else.
 * The check runs on launch and every 6 hours, never on the prompt's path.
 *
 * Step 16 (the update notice UI) reads `Updater.getStatus()` and
 * `Updater.onStatus()`, and starts the loop with `Updater.start()`.
 */
import crypto from "node:crypto"
import fs from "node:fs"
import path from "node:path"
import { spawn as nodeSpawn } from "node:child_process"
import { isSelfHosted } from "@/shared/selfhost/offline"
import { gt, lt } from "@/shared/utils/semver"
import { ReleaseFeed } from "./feed"
import { Installation } from "./index"

export namespace Updater {
  export type Method = Installation.Method

  /** What this install does with an update. */
  export type Action = "auto-binary" | "auto-npm" | "command" | "managed" | "none"

  export type State =
    | "idle" // no check yet this launch
    | "checking"
    | "off" // self-hosted, dev build, or auto-update off with nothing required
    | "key-missing" // feed key not set: fail closed
    | "error" // feed unreachable / signature invalid / unreadable / download refused
    | "paused" // feed says paused
    | "current" // up to date
    | "available" // newer version; the user runs `command`
    | "downloading"
    | "staged" // binary verified and staged; swaps on exit
    | "scheduled" // npm install runs on exit
    | "managed" // Allternit Desktop owns this copy

  export interface Status {
    state: State
    method: Method | "pending"
    action: Action
    channel: ReleaseFeed.Channel
    current: string
    latest?: string
    minVersion?: string
    /** Running version is below the feed's minVersion. */
    required: boolean
    /** The exact command to run, for command methods (and as a fallback hint). */
    command?: string
    /** One plain line for the notice / doctor. */
    message: string
    /** Machine reason for error / key-missing states (a ReleaseFeed.FailReason or a download reason). */
    reason?: string
    highlights: string[]
    notesUrl?: string
    lastCheckAt?: string
    lastUpdateAt?: string
    lastUpdateFrom?: string
    lastUpdateTo?: string
  }

  export interface Options {
    /** Install method, or a resolver (defaults to Installation.method()). */
    method?: Method | (() => Promise<Method>)
    /** Running version (defaults to Installation.VERSION). */
    current?: string
    /** Feed artifact key, e.g. darwin-arm64 (defaults to process.platform-process.arch). */
    platform?: string
    /** Installed binary path (defaults to process.execPath, resolved through symlinks). */
    execPath?: string
    env?: NodeJS.ProcessEnv
    /** Where update.json lives (defaults to gizzi's state dir). */
    stateDir?: string
    fetch?: typeof fetch
    feed?: Omit<ReleaseFeed.FetchOptions, "fetch">
    now?: () => Date
    /** Starts the detached npm install after exit (tests replace it). */
    spawn?: (command: string, args: string[]) => void
    /** Announces a newer version on the bus (defaults to Installation.announceUpdate). */
    announce?: (version: string) => void | Promise<void>
  }

  // ---------------------------------------------------------------------------
  // Status (what step 16 reads)
  // ---------------------------------------------------------------------------

  let status: Status = {
    state: "idle",
    method: "pending",
    action: "none",
    channel: "stable",
    current: Installation.VERSION,
    required: false,
    message: "",
    highlights: [],
  }
  const listeners = new Set<(s: Status) => void>()

  /** Latest status snapshot. Never blocks, never touches the network. */
  export function getStatus(): Status {
    return status
  }

  /** Called with every status change. Returns an unsubscribe function. */
  export function onStatus(listener: (s: Status) => void): () => void {
    listeners.add(listener)
    return () => listeners.delete(listener)
  }

  function setStatus(next: Status): Status {
    status = next
    for (const l of [...listeners]) {
      try {
        l(next)
      } catch {
        // a broken listener never breaks the updater
      }
    }
    return next
  }

  // ---------------------------------------------------------------------------
  // Per-method policy
  // ---------------------------------------------------------------------------

  export function actionFor(method: Method): Action {
    switch (method) {
      case "curl":
        return "auto-binary"
      case "npm":
        return "auto-npm"
      case "desktop":
        return "managed"
      case "pnpm":
      case "bun":
      case "yarn":
      case "brew":
      case "scoop":
      case "choco":
      case "unknown":
        return "command"
      default:
        return "none"
    }
  }

  const NPM_PACKAGE = "@allternit/gizzi-code"
  const SAFE_VERSION = /^[0-9A-Za-z.+-]+$/
  const SAFE_NAME = /^[0-9A-Za-z@/._-]+$/

  /** npm spec from the feed, accepted only when it is our package at a plain version. */
  export function npmSpec(version: string, feed?: Pick<ReleaseFeed.Feed, "packages">): string | undefined {
    const spec = feed?.packages?.npm ?? `${NPM_PACKAGE}@${version}`
    const at = spec.lastIndexOf("@")
    if (at <= 0 || spec.slice(0, at) !== NPM_PACKAGE || !SAFE_VERSION.test(spec.slice(at + 1))) return undefined
    return spec
  }

  export function commandFor(method: Method, version: string, feed?: Pick<ReleaseFeed.Feed, "packages">): string | undefined {
    const spec = npmSpec(version, feed) ?? `${NPM_PACKAGE}@${version}`
    const brew = feed?.packages?.brew && SAFE_NAME.test(feed.packages.brew) ? feed.packages.brew : "gizzi-code"
    const scoop = feed?.packages?.scoop && SAFE_NAME.test(feed.packages.scoop) ? feed.packages.scoop : "gizzi-code"
    switch (method) {
      case "npm":
        return `npm i -g ${spec}`
      case "pnpm":
        return `pnpm add -g ${spec}`
      case "bun":
        return `bun add -g ${spec}`
      case "yarn":
        return `yarn global add ${spec}`
      case "brew":
        return `brew upgrade ${brew}`
      case "scoop":
        return `scoop update ${scoop}`
      case "choco":
        return "choco upgrade gizzi -y"
      case "curl":
      case "unknown":
        return "gizzi update"
      default:
        return undefined
    }
  }

  export function platformKey(platform = process.platform, arch = process.arch): string {
    return `${platform}-${arch}`
  }

  /**
   * The installed binary to replace: the real path of `execPath`, only when it
   * is a gizzi binary (a dev run under `bun` must never replace bun).
   */
  export function binaryTarget(execPath = process.execPath): string | undefined {
    let real: string
    try {
      real = fs.realpathSync(execPath)
    } catch {
      return undefined
    }
    return /^gizzi/i.test(path.basename(real)) ? real : undefined
  }

  function autoUpdateOff(env: NodeJS.ProcessEnv): boolean {
    const on = (v?: string) => ["1", "true", "yes", "on"].includes((v ?? "").trim().toLowerCase())
    return on(env.GIZZI_DISABLE_AUTOUPDATE) || on(env.DISABLE_AUTOUPDATER)
  }

  // ---------------------------------------------------------------------------
  // Download + verify
  // ---------------------------------------------------------------------------

  export type DownloadResult = { ok: true; path: string } | { ok: false; reason: string }

  /** Whole-download ceiling: a release binary on a slow line, not forever. */
  const DOWNLOAD_TIMEOUT_MS = 15 * 60 * 1000

  /**
   * Download `artifact` to `dest`, hashing as it streams. A size or sha256
   * that differs from the signed feed deletes the file and refuses.
   */
  export async function downloadVerified(
    artifact: ReleaseFeed.Artifact,
    dest: string,
    fetchImpl: typeof fetch = fetch,
  ): Promise<DownloadResult> {
    const tmp = `${dest}.part`
    try {
      fs.mkdirSync(path.dirname(dest), { recursive: true })
      const res = await fetchImpl(artifact.url, { signal: AbortSignal.timeout(DOWNLOAD_TIMEOUT_MS) } as RequestInit)
      if (!res.ok || !res.body) return { ok: false, reason: `download failed (${res.status})` }
      const hash = crypto.createHash("sha256")
      const out = fs.createWriteStream(tmp, { mode: 0o755 })
      let size = 0
      const reader = res.body.getReader()
      try {
        for (;;) {
          const { done, value } = await reader.read()
          if (done) break
          size += value.byteLength
          // The byte host is untrusted: stop as soon as it sends more than the signed size.
          if (artifact.size > 0 && size > artifact.size) throw new Error("larger than the signed size")
          hash.update(value)
          if (!out.write(value)) await new Promise<void>((r) => out.once("drain", () => r()))
        }
      } finally {
        await new Promise<void>((resolve, reject) => {
          out.once("error", reject)
          out.end(() => resolve())
        })
      }
      const digest = hash.digest("hex")
      if (digest.toLowerCase() !== artifact.sha256.toLowerCase()) {
        fs.rmSync(tmp, { force: true })
        return { ok: false, reason: "sha256 mismatch" }
      }
      if (artifact.size > 0 && size !== artifact.size) {
        fs.rmSync(tmp, { force: true })
        return { ok: false, reason: "size mismatch" }
      }
      fs.renameSync(tmp, dest)
      if (process.platform !== "win32") fs.chmodSync(dest, 0o755)
      return { ok: true, path: dest }
    } catch (err) {
      fs.rmSync(tmp, { force: true })
      return { ok: false, reason: `download failed (${err instanceof Error ? err.message : String(err)})` }
    }
  }

  function sha256File(file: string): string {
    const hash = crypto.createHash("sha256")
    const fd = fs.openSync(file, "r")
    try {
      const buf = Buffer.allocUnsafe(1 << 20)
      for (;;) {
        const n = fs.readSync(fd, buf, 0, buf.length, null)
        if (n <= 0) break
        hash.update(buf.subarray(0, n))
      }
    } finally {
      fs.closeSync(fd)
    }
    return hash.digest("hex")
  }

  function stageDirFor(target: string, version: string): string {
    return path.join(path.dirname(target), `.gizzi-update-${version}`)
  }

  function dropStaged(stateDir?: string): void {
    const saved = ReleaseFeed.readState(stateDir).staged
    if (saved?.dir) fs.rmSync(saved.dir, { recursive: true, force: true })
    ReleaseFeed.writeState({ staged: undefined }, stateDir)
  }

  /** The release archive's main binary, and helpers the installer puts beside it (cmd/gizzi-code/install). */
  const MAIN_BINARY = "gizzi-code"
  const HELPERS = ["allternit-factory", "allternit-files-mount"]

  function archiveKind(url: string): "tar.gz" | "zip" | "raw" {
    const u = url.toLowerCase().split("?")[0]!
    if (u.endsWith(".tar.gz") || u.endsWith(".tgz")) return "tar.gz"
    if (u.endsWith(".zip")) return "zip"
    return "raw"
  }

  function run(command: string, args: string[]): Promise<number> {
    return new Promise((resolve) => {
      const child = nodeSpawn(command, args, { stdio: "ignore", windowsHide: true })
      child.on("error", () => resolve(-1))
      child.on("exit", (code) => resolve(code ?? -1))
    })
  }

  /** Unpack a verified archive into `outDir` (tar on macOS/Linux; Windows 10+ tar reads zip). */
  export async function extractArchive(archive: string, outDir: string, kind: "tar.gz" | "zip", platform = process.platform): Promise<boolean> {
    fs.mkdirSync(outDir, { recursive: true })
    if (kind === "tar.gz") return (await run("tar", ["-xzf", archive, "-C", outDir])) === 0
    if (platform === "win32") return (await run("tar", ["-xf", archive, "-C", outDir])) === 0
    return (await run("unzip", ["-q", "-o", archive, "-d", outDir])) === 0
  }

  export type StageResult = { ok: true; staged: ReleaseFeed.StagedUpdate } | { ok: false; reason: string }

  /**
   * Download the artifact into a staging dir next to `target`, verify it
   * against the signed feed, unpack it, and list the files the swap moves.
   * Anything that fails removes the staging dir.
   */
  export async function stageArtifact(input: {
    artifact: ReleaseFeed.Artifact
    version: string
    target: string
    fetch?: typeof fetch
    platform?: NodeJS.Platform
    now?: () => Date
  }): Promise<StageResult> {
    const platform = input.platform ?? process.platform
    const exe = platform === "win32" ? ".exe" : ""
    const dir = stageDirFor(input.target, input.version)
    fs.rmSync(dir, { recursive: true, force: true })
    const kind = archiveKind(input.artifact.url)
    const download = path.join(dir, kind === "raw" ? `download${exe}` : `download.${kind}`)
    const dl = await downloadVerified(input.artifact, download, input.fetch ?? fetch)
    if (!dl.ok) {
      fs.rmSync(dir, { recursive: true, force: true })
      return dl
    }
    const files: ReleaseFeed.StagedFile[] = []
    if (kind === "raw") {
      files.push({ from: download, to: input.target, sha256: input.artifact.sha256.toLowerCase() })
    } else {
      const out = path.join(dir, "unpacked")
      if (!(await extractArchive(download, out, kind, platform))) {
        fs.rmSync(dir, { recursive: true, force: true })
        return { ok: false, reason: "archive could not be unpacked" }
      }
      fs.rmSync(download, { force: true })
      const main = path.join(out, MAIN_BINARY + exe)
      if (!fs.existsSync(main)) {
        fs.rmSync(dir, { recursive: true, force: true })
        return { ok: false, reason: `archive has no ${MAIN_BINARY}${exe}` }
      }
      files.push({ from: main, to: input.target, sha256: sha256File(main) })
      const installDir = path.dirname(input.target)
      for (const helper of HELPERS) {
        const from = path.join(out, helper + exe)
        if (fs.existsSync(from)) files.push({ from, to: path.join(installDir, helper + exe), sha256: sha256File(from) })
      }
    }
    return {
      ok: true,
      staged: { version: input.version, dir, target: input.target, files, stagedAt: (input.now?.() ?? new Date()).toISOString() },
    }
  }

  // ---------------------------------------------------------------------------
  // Swap (synchronous: it runs inside process.on("exit"))
  // ---------------------------------------------------------------------------

  /** Move one staged file over its destination. Windows renames the (possibly running) old file aside first. */
  function swapFile(file: ReleaseFeed.StagedFile, platform: NodeJS.Platform): void {
    if (platform === "win32") {
      const aside = `${file.to}.old-${Date.now()}`
      const had = fs.existsSync(file.to)
      if (had) fs.renameSync(file.to, aside)
      try {
        fs.renameSync(file.from, file.to)
      } catch (err) {
        if (had) fs.renameSync(aside, file.to)
        throw err
      }
      return
    }
    fs.chmodSync(file.from, 0o755)
    fs.renameSync(file.from, file.to)
  }

  /**
   * Put a staged, verified update in place of the installed binary (and its
   * helpers). Re-hashes every staged file first. Returns true when the main
   * binary was swapped. Never throws.
   */
  export function applyStaged(opts: { current?: string; stateDir?: string; platform?: NodeJS.Platform; now?: () => Date } = {}): boolean {
    const current = opts.current ?? Installation.VERSION
    const platform = opts.platform ?? process.platform
    const st = ReleaseFeed.readState(opts.stateDir)
    const staged = st.staged
    if (!staged) return false
    try {
      const [main, ...helpers] = staged.files ?? []
      const stale = st.paused || !main || (current !== "local" && !gt(staged.version, current))
      const intact = !stale && staged.files.every((f) => fs.existsSync(f.from) && sha256File(f.from).toLowerCase() === f.sha256.toLowerCase())
      if (!intact) {
        dropStaged(opts.stateDir)
        return false
      }
      swapFile(main!, platform)
      for (const helper of helpers) {
        try {
          swapFile(helper, platform)
        } catch {
          // a helper that cannot be replaced keeps its old copy; the main binary still updated
        }
      }
      ReleaseFeed.writeState(
        {
          staged: undefined,
          lastUpdateAt: (opts.now?.() ?? new Date()).toISOString(),
          lastUpdateFrom: current,
          lastUpdateTo: staged.version,
        },
        opts.stateDir,
      )
      fs.rmSync(staged.dir, { recursive: true, force: true })
      return true
    } catch {
      return false
    }
  }

  /** Windows: remove files renamed aside by earlier swaps (a still-locked one waits for a later launch). */
  export function cleanupAside(target: string | undefined): void {
    if (!target) return
    try {
      const dir = path.dirname(target)
      for (const name of fs.readdirSync(dir)) {
        if (/\.old-\d+$/.test(name)) fs.rmSync(path.join(dir, name), { force: true })
      }
    } catch {
      // best effort
    }
  }

  function defaultSpawn(command: string, args: string[]): void {
    const child = nodeSpawn(command, args, {
      detached: true,
      stdio: "ignore",
      shell: process.platform === "win32",
      windowsHide: true,
    })
    child.on("error", () => {})
    child.unref()
  }

  /** npm path: start `npm i -g <spec>` detached so it runs after gizzi exits. */
  export function runPendingPackage(opts: { stateDir?: string; spawn?: Options["spawn"] } = {}): boolean {
    const pending = ReleaseFeed.readState(opts.stateDir).pendingPackage
    if (!pending) return false
    const spec = npmSpec(pending.version)
    if (!spec) return false
    try {
      ;(opts.spawn ?? defaultSpawn)("npm", ["i", "-g", spec])
      return true
    } catch {
      return false
    }
  }

  // ---------------------------------------------------------------------------
  // One check
  // ---------------------------------------------------------------------------

  async function resolveMethod(m: Options["method"]): Promise<Method> {
    if (typeof m === "function") return m()
    if (m) return m
    return Installation.method()
  }

  function base(opts: Options, method: Method, ch: ReleaseFeed.Channel, current: string): Status {
    const st = ReleaseFeed.readState(opts.stateDir)
    return {
      state: "idle",
      method,
      action: actionFor(method),
      channel: ch,
      current,
      required: false,
      message: "",
      highlights: st.highlights ?? [],
      notesUrl: st.notesUrl,
      lastCheckAt: st.lastCheckAt,
      lastUpdateAt: st.lastUpdateAt,
      lastUpdateFrom: st.lastUpdateFrom,
      lastUpdateTo: st.lastUpdateTo,
    }
  }

  let inFlight: Promise<Status> | undefined

  /** Run one feed check and act on it. Concurrent calls share one check. Never throws. */
  export function check(opts: Options = {}): Promise<Status> {
    if (!inFlight) {
      inFlight = runCheck(opts)
        .catch((err) =>
          setStatus({
            ...status,
            state: "error",
            reason: err instanceof Error ? err.message : String(err),
            message: "Update check failed.",
          }),
        )
        .finally(() => {
          inFlight = undefined
        })
    }
    return inFlight
  }

  async function runCheck(opts: Options): Promise<Status> {
    const env = opts.env ?? process.env
    const now = opts.now ?? (() => new Date())
    const current = opts.current ?? Installation.VERSION
    const ch = opts.feed?.channel ?? ReleaseFeed.channel(env, opts.stateDir)
    const method = await resolveMethod(opts.method)
    const action = actionFor(method)
    const b = base(opts, method, ch, current)

    if (action === "managed") {
      return setStatus({ ...b, state: "managed", message: "Gizzi is managed by Allternit Desktop. Desktop updates it." })
    }
    if (isSelfHosted()) {
      return setStatus({ ...b, state: "off", message: "Update checks are off in self-hosted mode." })
    }
    if (current === "local") {
      return setStatus({ ...b, state: "off", message: "Development build: updates are off." })
    }

    setStatus({ ...b, state: "checking", message: "Checking for updates…" })
    const result = await ReleaseFeed.fetchFeed({
      ...opts.feed,
      channel: ch,
      base: opts.feed?.base ?? ReleaseFeed.feedBase(env),
      publicKey: opts.feed?.publicKey ?? ReleaseFeed.activeKey(env),
      fetch: opts.fetch,
    })
    const st = ReleaseFeed.recordCheck(result, now(), opts.stateDir)
    const after = { ...b, lastCheckAt: st.lastCheckAt, highlights: st.highlights ?? [], notesUrl: st.notesUrl }

    if (!result.ok) {
      if (result.reason === "feed key not set") {
        return setStatus({ ...after, state: "key-missing", reason: result.reason, message: "Updates are off: feed key not set." })
      }
      return setStatus({ ...after, state: "error", reason: result.reason, message: `Update check failed: ${result.reason}.` })
    }

    const feed = result.feed
    if (feed.paused) {
      dropStaged(opts.stateDir)
      ReleaseFeed.writeState({ pendingPackage: undefined }, opts.stateDir)
      return setStatus({ ...after, state: "paused", latest: feed.version, minVersion: feed.minVersion, message: "Updates are paused by Allternit." })
    }

    const newer = gt(feed.version, current)
    const required = newer && !!feed.minVersion && lt(current, feed.minVersion)
    const command = commandFor(method, feed.version, feed)
    const withFeed: Status = { ...after, latest: feed.version, minVersion: feed.minVersion, required, command }

    if (!newer) {
      dropStaged(opts.stateDir)
      return setStatus({ ...withFeed, state: "current", command: undefined, message: `Gizzi ${current} is up to date.` })
    }

    try {
      await (opts.announce ?? Installation.announceUpdate)(feed.version)
    } catch {
      // the bus is optional (no Instance outside the server)
    }

    const label = required ? `Gizzi ${feed.version} is required` : `Gizzi ${feed.version} is available`
    if (autoUpdateOff(env) && !required) {
      return setStatus({ ...withFeed, state: "available", message: `${label}. Run: ${command ?? "gizzi update"}` })
    }

    if (action === "auto-npm") {
      const spec = npmSpec(feed.version, feed)
      if (!spec) {
        return setStatus({ ...withFeed, state: "available", reason: "npm spec refused", message: `${label}. Run: ${command}` })
      }
      ReleaseFeed.writeState(
        { pendingPackage: { version: feed.version, from: current, command: `npm i -g ${spec}`, startedAt: now().toISOString() } },
        opts.stateDir,
      )
      return setStatus({ ...withFeed, state: "scheduled", message: `${label}. It installs when you quit.` })
    }

    if (action === "auto-binary") {
      const target = binaryTarget(opts.execPath)
      const artifact = feed.artifacts[opts.platform ?? platformKey()]
      if (!target || !artifact) {
        return setStatus({
          ...withFeed,
          state: "available",
          reason: !target ? "installed binary not found" : "no artifact for this platform",
          message: `${label}. Run: gizzi update`,
        })
      }
      const saved = ReleaseFeed.readState(opts.stateDir).staged
      if (saved && saved.version === feed.version && saved.target === target && saved.files?.every((f) => fs.existsSync(f.from))) {
        return setStatus({ ...withFeed, state: "staged", message: `${label}. It installs when you quit.` })
      }
      if (saved) dropStaged(opts.stateDir)
      setStatus({ ...withFeed, state: "downloading", message: `Downloading Gizzi ${feed.version}…` })
      const result = await stageArtifact({ artifact, version: feed.version, target, fetch: opts.fetch, now })
      if (!result.ok) {
        return setStatus({ ...withFeed, state: "error", reason: result.reason, message: `Update refused: ${result.reason}.` })
      }
      ReleaseFeed.writeState({ staged: result.staged }, opts.stateDir)
      return setStatus({ ...withFeed, state: "staged", message: `${label}. It installs when you quit.` })
    }

    // command methods (brew / scoop / choco / pnpm / bun / yarn / unknown): print, never self-replace
    return setStatus({ ...withFeed, state: "available", message: `${label}. Run: ${command ?? "gizzi update"}` })
  }

  // ---------------------------------------------------------------------------
  // Loop: launch + every 6 h, exit hook
  // ---------------------------------------------------------------------------

  let running: { stop: () => void } | undefined

  /**
   * Start the updater for this process (idempotent). Applies an update staged
   * by an earlier run whose exit hook never ran, confirms a finished npm
   * update, checks shortly after launch and then every 6 hours on unref'd
   * timers, and swaps / starts npm on exit. Never awaits the network.
   */
  export function start(opts: Options = {}): { stop: () => void } {
    if (running) return running
    const current = opts.current ?? Installation.VERSION
    const now = opts.now ?? (() => new Date())

    const st = ReleaseFeed.readState(opts.stateDir)
    if (st.pendingPackage) {
      if (st.pendingPackage.version === current) {
        ReleaseFeed.writeState(
          { pendingPackage: undefined, lastUpdateAt: now().toISOString(), lastUpdateFrom: st.pendingPackage.from, lastUpdateTo: current },
          opts.stateDir,
        )
      } else {
        // the next check schedules it again if it is still newer
        ReleaseFeed.writeState({ pendingPackage: undefined }, opts.stateDir)
      }
    }
    if (st.staged) applyStaged({ current, stateDir: opts.stateDir, now })
    if (process.platform === "win32") cleanupAside(binaryTarget(opts.execPath))

    // Reads update.json, not the in-memory status: a later failed check must not cancel a verified staged update.
    const onExit = () => {
      if (!applyStaged({ current, stateDir: opts.stateDir, now })) runPendingPackage({ stateDir: opts.stateDir, spawn: opts.spawn })
    }
    process.once("exit", onExit)

    const first = setTimeout(() => void check(opts), 2_000)
    const every = setInterval(() => void check(opts), ReleaseFeed.CHECK_INTERVAL_MS)
    first.unref?.()
    every.unref?.()

    running = {
      stop: () => {
        clearTimeout(first)
        clearInterval(every)
        process.removeListener("exit", onExit)
        running = undefined
      },
    }
    return running
  }

  /** Test hook: forget the in-memory status and listeners. */
  export function resetForTests(): void {
    running?.stop()
    listeners.clear()
    inFlight = undefined
    status = { state: "idle", method: "pending", action: "none", channel: "stable", current: Installation.VERSION, required: false, message: "", highlights: [] }
  }
}
