// Cloud runtime relay for a paired gizzi engine (Start anywhere, step 10).
//
// Allternit Desktop keeps this machine reachable from the phone and the web by
// holding a WebSocket to cloud-api's runtime relay. Without Desktop, the gizzi
// engine (`gizzi serve`, or the unattended daemon) opens the same relay itself
// when `gizzi pair` left a runtime identity. Port of
// cmd/agent-daemon/src/index.ts (connect, authenticate, forward relayed HTTP
// and WebSocket traffic, reconnect with backoff, token rotation), with the
// routing and credential rules of Desktop's relay (auth-manager.ts,
// relay-gizzi-credential.ts, human-proof.ts).
//
// Wire contract (cmd/allternit-cloud-api/src/routes/runtime_relay.rs):
//   connect   wss://<cloud>/api/v1/runtime-relay/connect/:runtimeId
//   ->        { type:"authenticate", runtime_id, device_token, remote_start }
//   <-        authenticated | request | socket_open | socket_data | socket_close | ping
//   ->        response_start / response_chunk / response_end | socket_ready / socket_data / socket_close | pong
// `remote_start` advertises the Remote start switch live (D6). Absent = off,
// and the cloud answers relayed session starts with 403 `remote_start_off`.
//
// One relay per machine: a pid-checked lock at ~/.config/allternit/relay.lock
// ({pid, owner, startedAt}). gizzi connects only when no live process holds it
// and yields to Desktop. Desktop does not write the lock yet, so a running
// Desktop is also detected by its allternit-api gateway answering /health on
// loopback (127.0.0.1:8013 unless ALLTERNIT_API_URL says otherwise), and a
// gizzi Desktop spawned itself (GIZZI_PARENT_LIFELINE) never connects.
import os from "node:os"
import path from "node:path"
import { closeSync, mkdirSync, openSync, readFileSync, statSync, unlinkSync, writeFileSync, writeSync } from "node:fs"
import WebSocket, { type RawData } from "ws"
import { Log } from "@/shared/util/log"
import { Flag } from "@/runtime/context/flag/flag"
import { Pairing } from "@/runtime/services/pairing/pairing"
import { isSelfHosted } from "@/shared/selfhost/offline"
import { gatewayBase } from "@/shared/constants/allternitGateway"

export namespace RelayConnector {
  const log = Log.create({ service: "relay-connector" })

  const TICK_MS = 30_000
  const WATCHDOG_MS = 75_000
  const ROTATE_CHECK_MS = 60 * 60 * 1000
  const MAX_BACKOFF_MS = 30_000
  const PROBE_TIMEOUT_MS = 800

  // ── Lock ─────────────────────────────────────────────────────────────────

  export type Owner = "gizzi" | "desktop"
  export interface LockInfo {
    pid: number
    owner: Owner
    startedAt: string
    /** Set by gizzi while its relay is authenticated (read by `/remote`). */
    connected?: boolean
  }

  export function lockPath(): string {
    return process.env.ALLTERNIT_RELAY_LOCK_PATH || path.join(os.homedir(), ".config", "allternit", "relay.lock")
  }

  /** True when a process with this pid exists (EPERM = exists, owned by another user). */
  export function pidAlive(pid: number): boolean {
    if (!Number.isInteger(pid) || pid <= 0) return false
    try {
      process.kill(pid, 0)
      return true
    } catch (err) {
      return (err as NodeJS.ErrnoException).code === "EPERM"
    }
  }

  export function readLock(file = lockPath()): LockInfo | undefined {
    try {
      const parsed = JSON.parse(readFileSync(file, "utf8")) as Partial<LockInfo>
      if (typeof parsed.pid !== "number" || (parsed.owner !== "gizzi" && parsed.owner !== "desktop")) return undefined
      return {
        pid: parsed.pid,
        owner: parsed.owner,
        startedAt: String(parsed.startedAt ?? ""),
        connected: parsed.connected === true,
      }
    } catch {
      return undefined
    }
  }

  /** The lock's holder when that process is still alive. */
  export function liveHolder(file = lockPath()): LockInfo | undefined {
    const held = readLock(file)
    return held && pidAlive(held.pid) ? held : undefined
  }

  /** A lock file that exists but does not parse yet: another process may be mid-write. */
  function freshUnreadable(file: string): boolean {
    try {
      return Date.now() - statSync(file).mtimeMs < 5_000
    } catch {
      return false
    }
  }

  /**
   * Take the lock for this process. False when a live process (other than
   * this one) holds it. A lock left by a dead pid, or one that never parsed,
   * is removed and taken.
   */
  export function acquireLock(owner: Owner, file = lockPath()): boolean {
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        mkdirSync(path.dirname(file), { recursive: true })
        const fd = openSync(file, "wx", 0o600)
        try {
          const info: LockInfo = { pid: process.pid, owner, startedAt: new Date().toISOString() }
          writeSync(fd, JSON.stringify(info))
        } finally {
          closeSync(fd)
        }
        return true
      } catch (err) {
        if ((err as NodeJS.ErrnoException).code !== "EEXIST") {
          log.warn("relay lock unavailable", { file, error: err instanceof Error ? err.message : String(err) })
          return false
        }
        const held = readLock(file)
        if (held?.pid === process.pid) return true
        if (held && pidAlive(held.pid)) return false
        if (!held && freshUnreadable(file)) return false
        try {
          unlinkSync(file)
        } catch {
          // someone else removed it first; the retry decides
        }
      }
    }
    return false
  }

  /** True while the lock file names this process. */
  export function ownsLock(file = lockPath()): boolean {
    return readLock(file)?.pid === process.pid
  }

  /** Remove the lock, only when this process holds it. */
  export function releaseLock(file = lockPath()): void {
    if (!ownsLock(file)) return
    try {
      unlinkSync(file)
    } catch {
      // already gone
    }
  }

  function markConnected(connected: boolean, file = lockPath()): void {
    const held = readLock(file)
    if (held?.pid !== process.pid || held.connected === connected) return
    try {
      writeFileSync(file, JSON.stringify({ ...held, connected }), { mode: 0o600 })
    } catch {
      // status only; the lock itself is unchanged
    }
  }

  // ── Desktop detection ────────────────────────────────────────────────────

  function isLoopbackUrl(url: string): boolean {
    try {
      const host = new URL(url).hostname
      return host === "127.0.0.1" || host === "localhost" || host === "[::1]" || host === "::1"
    } catch {
      return false
    }
  }

  /** Desktop's allternit-api gateway answers on loopback. */
  export async function desktopGatewayUp(): Promise<boolean> {
    const base = gatewayBase()
    if (!isLoopbackUrl(base)) return false
    try {
      const response = await fetch(`${base}/health`, { signal: AbortSignal.timeout(PROBE_TIMEOUT_MS) })
      return response.ok
    } catch {
      return false
    }
  }

  /** Desktop holds (or is running) the relay for this machine. */
  export async function desktopHoldsRelay(): Promise<boolean> {
    if (liveHolder()?.owner === "desktop") return true
    return desktopGatewayUp()
  }

  export type Reachability = { reachable: boolean; via?: "gizzi" | "desktop" }

  /** Who keeps this machine reachable from the phone and the web right now (read by `/remote`). */
  export async function reachability(): Promise<Reachability> {
    const holder = liveHolder()
    if (holder?.owner === "desktop") return { reachable: true, via: "desktop" }
    if (holder?.owner === "gizzi" && holder.connected) return { reachable: true, via: "gizzi" }
    if (await desktopGatewayUp()) return { reachable: true, via: "desktop" }
    return { reachable: false }
  }

  // ── Settings ─────────────────────────────────────────────────────────────

  /** Remote start switch: config `remoteStart` (default true); ALLTERNIT_REMOTE_START=0 turns it off. */
  export async function remoteStartEnabled(): Promise<boolean> {
    const env = process.env.ALLTERNIT_REMOTE_START?.trim().toLowerCase()
    if (env === "0" || env === "false" || env === "off") return false
    try {
      const { Config } = await import("@/runtime/context/config/config")
      const config = await Config.getGlobal()
      return config.remoteStart !== false
    } catch {
      return true
    }
  }

  // ── Routing (mirrors Desktop's relayLocalUrl) ────────────────────────────

  const HTTP_PREFIXES = [
    "/api/", "/viz", "/sandbox", "/vm-session", "/stream",
    "/terminal", "/mcp", "/platform", "/metrics", "/alabs", "/cowork",
    "/webhooks", "/status", "/health",
    "/ws", "/panes",
    "/v1/",
  ]
  const SOCKET_PREFIXES = [
    "/api/", "/viz", "/sandbox", "/vm-session", "/stream",
    "/terminal", "/mcp", "/platform", "/metrics", "/cowork",
    "/webhooks", "/ws", "/panes", "/status", "/health",
    "/v1/pty/",
    "/v1/host/",
  ]
  const HUMAN_PROOF_HEADER = "X-Allternit-Human-Proof"
  const PROOF_PARAM = "allternit_person"

  /** gizzi routes allternit-api does not serve (same set as Desktop and agent-daemon). */
  export function isGizziOwnedRelayPath(requestPath: string): boolean {
    return /^\/v1\/(pty|file|cron|automations|mcp|vcs|path)(\/|\?|$)/.test(requestPath) || /^\/v1\/event(\?|$)/.test(requestPath)
  }

  /** Where a relayed request goes: this engine or the local gateway. */
  export function localTarget(requestPath: string, engine: string, gateway: string): { url: string; gizzi: boolean } {
    if (requestPath.startsWith("/api/v1/remote-control/")) {
      return { url: `${engine}${requestPath.replace("/api/v1/remote-control/", "/v1/remote-control/")}`, gizzi: true }
    }
    if (/^\/api\/v1\/providers\/?(\?|$)/.test(requestPath)) return { url: `${engine}/v1/provider`, gizzi: true }
    if (
      requestPath.startsWith("/v1/remote-control/") ||
      requestPath.startsWith("/v1/provider") ||
      requestPath.startsWith("/v1/agent") ||
      requestPath.startsWith("/v1/permission") ||
      requestPath.startsWith("/v1/question") ||
      /^\/v1\/session(\/|\?|$)/.test(requestPath) ||
      isGizziOwnedRelayPath(requestPath)
    ) {
      return { url: `${engine}${requestPath}`, gizzi: true }
    }
    return { url: `${gateway}${requestPath}`, gizzi: false }
  }

  function safePath(requestPath: string): boolean {
    return requestPath.startsWith("/") && !requestPath.includes("..") && !requestPath.includes("://")
  }

  function stripProofParam(requestPath: string): string {
    const url = new URL(requestPath, "http://relay.invalid")
    if (!url.searchParams.has(PROOF_PARAM)) return requestPath
    url.searchParams.delete(PROOF_PARAM)
    return `${url.pathname}${url.search}`
  }

  /** Basic auth for this engine when it runs with a password (the unattended daemon does). */
  function engineAuthorization(): string | undefined {
    const password = process.env.GIZZI_SERVER_PASSWORD
    if (!password) return undefined
    const username = process.env.GIZZI_SERVER_USERNAME ?? "gizzi"
    return `Basic ${Buffer.from(`${username}:${password}`).toString("base64")}`
  }

  // ── Connection ───────────────────────────────────────────────────────────

  type Identity = Pairing.Stored & { runtimeId: string; deviceToken: string }

  let started = false
  let engineUrl = ""
  let holding = false
  let ticking = false
  let tickTimer: ReturnType<typeof setInterval> | undefined
  let retryTimer: ReturnType<typeof setTimeout> | undefined
  let watchdogTimer: ReturnType<typeof setInterval> | undefined
  let relay: WebSocket | null = null
  let identity: Identity | null = null
  let retryMs = 1_000
  let lastMessageAt = 0
  let lastRotateCheck = 0
  const localSockets = new Map<string, WebSocket>()

  function platformBase(): string {
    return Flag.GIZZI_PLATFORM_API_URL.replace(/\/+$/, "")
  }

  async function loadIdentity(): Promise<Identity | null> {
    const stored = await Pairing.load().catch(() => undefined)
    if (!Pairing.tokenUsable(stored) || !stored.runtimeId) return null
    return stored as Identity
  }

  function send(socket: WebSocket, message: unknown): boolean {
    if (socket.readyState !== WebSocket.OPEN) return false
    socket.send(JSON.stringify(message))
    return true
  }

  function sendJson(socket: WebSocket, requestId: string, status: number, payload: unknown) {
    send(socket, { type: "response_start", request_id: requestId, status, headers: { "content-type": "application/json" } })
    send(socket, {
      type: "response_chunk",
      request_id: requestId,
      body: Buffer.from(JSON.stringify(payload)).toString("base64"),
      body_encoding: "base64",
    })
    send(socket, { type: "response_end", request_id: requestId })
  }

  /** Engine started: open the relay when paired, not self-hosted, and Desktop is not holding it. */
  export async function start(opts: { engineUrl: string }): Promise<void> {
    if (started) return
    const off = process.env.GIZZI_RELAY_CONNECTOR?.trim().toLowerCase()
    if (off === "0" || off === "false") return
    if (isSelfHosted()) return
    // Desktop spawned this engine; Desktop's own relay serves it.
    if (process.env.GIZZI_PARENT_LIFELINE) return
    if (!(await loadIdentity())) {
      log.info("not paired; the cloud relay stays closed (run `gizzi pair` to make this machine reachable)")
      return
    }
    started = true
    engineUrl = opts.engineUrl.replace(/\/+$/, "")
    process.once("exit", () => releaseLock())
    void tick()
    tickTimer = setInterval(() => void tick(), TICK_MS)
    tickTimer.unref?.()
  }

  export function stop(): void {
    if (!started) return
    started = false
    if (tickTimer) clearInterval(tickTimer)
    tickTimer = undefined
    yieldRelay()
  }

  export function active(): boolean {
    return holding && !!relay && relay.readyState === WebSocket.OPEN
  }

  function yieldRelay() {
    holding = false
    if (retryTimer) clearTimeout(retryTimer)
    retryTimer = undefined
    stopWatchdog()
    const socket = relay
    relay = null
    socket?.close()
    closeLocalSockets()
    releaseLock()
  }

  function closeLocalSockets() {
    for (const local of localSockets.values()) local.close(1012, "Relay disconnected")
    localSockets.clear()
  }

  async function tick(): Promise<void> {
    if (!started || ticking) return
    ticking = true
    try {
      if (holding) {
        if (!ownsLock() || (await desktopHoldsRelay())) {
          log.info("yielding the cloud relay to Allternit Desktop")
          yieldRelay()
          return
        }
        await heartbeat()
        await rotateIfDue()
        return
      }
      if (await desktopHoldsRelay()) return
      if (!acquireLock("gizzi")) return
      holding = true
      retryMs = 1_000
      await connect()
    } catch (err) {
      log.warn("relay tick failed", { error: err instanceof Error ? err.message : String(err) })
    } finally {
      ticking = false
    }
  }

  async function heartbeat(): Promise<void> {
    if (!identity) return
    try {
      const response = await fetch(
        `${platformBase()}/api/v1/runtime-devices/${encodeURIComponent(identity.runtimeId)}/heartbeat`,
        { method: "POST", headers: { authorization: `Bearer ${identity.deviceToken}` }, signal: AbortSignal.timeout(10_000) },
      )
      if (response.status === 401 || response.status === 403) {
        log.warn("this runtime was revoked; closing the cloud relay. Run `gizzi pair --force` to pair it again.")
        stop()
      }
    } catch {
      // transient; the relay itself is the presence signal
    }
  }

  async function rotateIfDue(): Promise<void> {
    if (Date.now() - lastRotateCheck < ROTATE_CHECK_MS) return
    lastRotateCheck = Date.now()
    // Same rotation as agent-daemon: a new token reconnects the relay with it.
    if (await Pairing.rotateIfNeeded()) relay?.close()
  }

  async function connect(): Promise<void> {
    if (!started || !holding || relay) return
    if (Date.now() - lastRotateCheck >= ROTATE_CHECK_MS) {
      lastRotateCheck = Date.now()
      await Pairing.rotateIfNeeded()
    }
    identity = await loadIdentity()
    if (!identity) {
      log.warn("paired token is no longer usable; closing the cloud relay. Run `gizzi pair` to re-pair.")
      stop()
      return
    }
    const remoteStart = await remoteStartEnabled()
    if (!started || !holding || relay) return
    const url = new URL(`${platformBase()}/api/v1/runtime-relay/connect/${encodeURIComponent(identity.runtimeId)}`)
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:"
    const socket = new WebSocket(url)
    relay = socket
    lastMessageAt = Date.now()
    socket.on("open", () => {
      if (!identity) return socket.close()
      send(socket, {
        type: "authenticate",
        runtime_id: identity.runtimeId,
        device_token: identity.deviceToken,
        remote_start: remoteStart,
      })
    })
    socket.on("message", (data) => {
      lastMessageAt = Date.now()
      let message: any
      try {
        message = JSON.parse(data.toString())
      } catch {
        return
      }
      if (message.type === "authenticated") {
        retryMs = 1_000
        markConnected(true)
        startWatchdog(socket)
        log.info("cloud relay connected", { runtimeId: identity?.runtimeId, remoteStart })
      } else if (message.type === "request") {
        void handleRequest(socket, message)
      } else if (message.type === "socket_open") {
        handleSocketOpen(socket, message)
      } else if (message.type === "socket_data") {
        handleSocketData(message)
      } else if (message.type === "socket_close") {
        handleSocketClose(message)
      } else if (message.type === "ping") {
        send(socket, { type: "pong" })
      }
    })
    socket.on("close", () => {
      // A socket a yield or reconnect already replaced can close late.
      if (relay !== socket) return
      relay = null
      stopWatchdog()
      markConnected(false)
      closeLocalSockets()
      if (!started || !holding) return
      const delay = retryMs
      retryMs = Math.min(retryMs * 2, MAX_BACKOFF_MS)
      retryTimer = setTimeout(() => void connect(), delay)
      retryTimer.unref?.()
    })
    socket.on("error", (error) => log.warn("cloud relay error", { error: error.message }))
  }

  // The cloud pings every 25s; a silently dead connection (sleep, network
  // change) never fires `close`, so drop it and reconnect (as Desktop does).
  function startWatchdog(socket: WebSocket) {
    stopWatchdog()
    watchdogTimer = setInterval(() => {
      if (relay !== socket) return stopWatchdog()
      if (Date.now() - lastMessageAt > WATCHDOG_MS) {
        log.warn("cloud relay heartbeat timed out; reconnecting")
        socket.terminate()
      }
    }, TICK_MS)
    watchdogTimer.unref?.()
  }

  function stopWatchdog() {
    if (watchdogTimer) clearInterval(watchdogTimer)
    watchdogTimer = undefined
  }

  function relayedHeaders(source: Headers | Record<string, string>, gizzi: boolean): Headers {
    const headers = new Headers(source as HeadersInit)
    headers.delete("host")
    // Another device's request never carries Desktop's person proof (D16).
    const proof = headers.get(HUMAN_PROOF_HEADER)?.trim().toLowerCase()
    if (proof?.startsWith("desktop")) headers.delete(HUMAN_PROOF_HEADER)
    headers.set("X-Allternit-Relayed", "1")
    headers.set("X-Allternit-User-Id", identity!.userId ?? "")
    headers.set("X-Allternit-User-Email", identity!.userEmail ?? "")
    if (identity!.organizationId) headers.set("X-Allternit-Tenant-Id", identity!.organizationId)
    else headers.delete("X-Allternit-Tenant-Id")
    if (gizzi) {
      // gizzi reads a non-`alt_` Bearer as a Clerk JWT and refuses the device
      // token; loopback needs no credential beyond this engine's own password.
      headers.delete("Authorization")
      headers.delete("X-Allternit-Desktop-Access-Token")
      const basic = engineAuthorization()
      if (basic) headers.set("Authorization", basic)
    } else {
      headers.set("Authorization", `Bearer ${identity!.deviceToken}`)
      headers.set("X-Allternit-Desktop-Access-Token", identity!.deviceToken)
    }
    return headers
  }

  async function handleRequest(socket: WebSocket, message: any): Promise<void> {
    const requestId = typeof message.request_id === "string" ? message.request_id : ""
    const method = typeof message.method === "string" ? message.method.toUpperCase() : "GET"
    const rawPath = typeof message.path === "string" ? message.path : ""
    if (!identity || !requestId) return
    if (!safePath(rawPath)) return sendJson(socket, requestId, 400, { error: "bad_path" })
    if (rawPath === "/host/shares/start" || /^\/host\/shares\/[A-Za-z0-9_-]{1,64}\/(?:resume|pause|stop)$/.test(rawPath)) {
      return sendJson(socket, requestId, 404, { error: "not_found", message: "This computer cannot start live links without Allternit Desktop." })
    }
    const isShareGate = /^\/shares(?:[/?]|$)/.test(rawPath)
    if (!isShareGate && !HTTP_PREFIXES.some((prefix) => rawPath.startsWith(prefix))) {
      return sendJson(socket, requestId, 404, { error: "not_found" })
    }
    const requestPath = stripProofParam(rawPath)
    const body =
      method === "GET" || method === "HEAD" || !message.body
        ? undefined
        : Buffer.from(String(message.body), message.body_encoding === "base64" ? "base64" : "utf8")
    try {
      const target = localTarget(requestPath, engineUrl, gatewayBase())
      let response: Response
      try {
        response = await fetch(target.url, { method, headers: relayedHeaders(message.headers || {}, target.gizzi), body })
      } catch (err) {
        // No Desktop means no gateway; gizzi serves its whole surface under /v1/.
        if (target.gizzi || !requestPath.startsWith("/v1/")) throw err
        response = await fetch(`${engineUrl}${requestPath}`, { method, headers: relayedHeaders(message.headers || {}, true), body })
      }
      const responseHeaders: Record<string, string> = {}
      for (const name of ["content-type", "cache-control", "content-disposition", "etag", "last-modified", "x-request-id"]) {
        const value = response.headers.get(name)
        if (value) responseHeaders[name] = value
      }
      send(socket, { type: "response_start", request_id: requestId, status: response.status, headers: responseHeaders })
      if (response.body) {
        const reader = response.body.getReader()
        for (;;) {
          const { done, value } = await reader.read()
          if (done) break
          if (!value?.byteLength) continue
          const sent = send(socket, {
            type: "response_chunk",
            request_id: requestId,
            body: Buffer.from(value).toString("base64"),
            body_encoding: "base64",
          })
          if (!sent) {
            await reader.cancel().catch(() => {})
            return
          }
        }
      }
      send(socket, { type: "response_end", request_id: requestId })
    } catch (error) {
      sendJson(socket, requestId, 502, { error: "runtime_proxy_error", message: String(error) })
    }
  }

  function handleSocketOpen(relaySocket: WebSocket, message: any): void {
    const socketId = typeof message.socket_id === "string" ? message.socket_id : ""
    const requestPath = typeof message.path === "string" ? message.path : ""
    if (!identity || !socketId || !safePath(requestPath) || !SOCKET_PREFIXES.some((prefix) => requestPath.startsWith(prefix))) return
    localSockets.get(socketId)?.close()
    const toGizzi = isGizziOwnedRelayPath(requestPath)
    const base = toGizzi ? engineUrl : gatewayBase()
    const headers: Record<string, string> = {}
    relayedHeaders({}, toGizzi).forEach((value, key) => {
      headers[key] = value
    })
    const local = new WebSocket(`${base.replace(/^http/, "ws")}${requestPath}`, { headers })
    localSockets.set(socketId, local)
    local.on("open", () => {
      send(relaySocket, { type: "socket_ready", socket_id: socketId })
    })
    local.on("message", (data: RawData, isBinary: boolean) => {
      send(relaySocket, {
        type: "socket_data",
        socket_id: socketId,
        body: isBinary ? Buffer.from(data as Buffer).toString("base64") : (data as Buffer).toString(),
        body_encoding: isBinary ? "base64" : "utf8",
      })
    })
    let localFailed = false
    local.on("close", (code: number, reason: Buffer) => {
      if (localSockets.get(socketId) === local) localSockets.delete(socketId)
      // 1006 (dropped / refused upgrade) must not go on the wire: send 1011.
      const wireCode = localFailed || code === 1006 ? 1011 : code === 1005 ? 1000 : code
      send(relaySocket, {
        type: "socket_close",
        socket_id: socketId,
        code: wireCode,
        reason: wireCode === 1011 && !reason.length ? "Local runtime socket failed" : reason.toString(),
      })
    })
    local.on("error", (error: Error) => {
      log.warn("local runtime socket failed", { error: error.message })
      localFailed = true
      local.close(1011, "Local runtime socket failed")
    })
  }

  function handleSocketData(message: any): void {
    const local = localSockets.get(message.socket_id)
    if (!local || local.readyState !== WebSocket.OPEN) return
    local.send(message.body_encoding === "base64" ? Buffer.from(String(message.body || ""), "base64") : String(message.body || ""))
  }

  function handleSocketClose(message: any): void {
    const local = localSockets.get(message.socket_id)
    if (!local) return
    localSockets.delete(message.socket_id)
    const code = Number(message.code) >= 1000 && Number(message.code) <= 4999 ? Number(message.code) : 1000
    local.close(code, String(message.reason || ""))
  }
}
