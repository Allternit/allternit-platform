// Runtime-device pairing client for the Allternit platform (device-code flow).
// Implements the wire contract of allternit-cloud-api src/routes/runtime_pairing.rs:
// this machine generates an Ed25519 identity, POSTs a pairing request, the user
// approves the user code in the browser (Clerk-authed), and we poll the exchange
// endpoint with a proof-of-possession signature until the platform issues a
// durable device token (allternit_runtime_..., ~90 days).
//
// Wire shapes coded against the Rust source:
//   POST /api/v1/runtime-pairings
//     req:  { name, runtimeType, hostname?, platform?, version?, publicKey, capabilities[] }
//     resp: 201 { pairingId, deviceCode, userCode, challenge, verificationUrl, expiresAt, pollIntervalSeconds }
//   POST /api/v1/runtime-pairings/exchange
//     req:  { pairingId, deviceCode, signature }
//     resp: 428 authorization_pending | 410 expired_token | 403 access_denied
//           | 200 { runtimeId, userId, userEmail, organizationId, deviceToken, tokenType, expiresAt, capabilities[] }
//   POST /api/v1/runtime-devices/:id/rotate
//     auth: Bearer <current device token> (no request body)
//     resp: 200 { runtimeId, deviceToken, tokenType, expiresAt }
//   publicKey  = base64url-no-pad of the raw 32-byte Ed25519 public key
//   signature  = base64url-no-pad of ed25519-sign("allternit-runtime-pairing:{pairingId}:{challenge}")
//   fingerprint = sha256 hex of the raw 32-byte public key
//
// State lives in <Global.Path.data>/runtime-device.json (mode 0600): the keypair
// (PKCS8 PEM private key), device identity, device token and its expiry. The
// keypair is reused across re-pairs; a new token simply replaces the old one.
import path from "path"
import os from "node:os"
import { execFileSync } from "node:child_process"
import fs from "node:fs/promises"
import { createHash, generateKeyPairSync, createPrivateKey, sign } from "node:crypto"
import open from "open"
import { Global } from "@/runtime/context/global/index"
import { Flag } from "@/runtime/context/flag/flag"
import { Log } from "@/shared/util/log"
import { Filesystem } from "@/runtime/util/filesystem"
import { Installation } from "@/shared/installation"

export namespace Pairing {
  /** The pairing request ended without approval: expired or denied. The CLI prints it without a stack trace. */
  export class PairingEnded extends Error {
    constructor(
      readonly reason: "expired" | "denied" | "refused",
      /** The platform's reason when it refused an approved pairing (e.g. the runtime limit). */
      readonly detail?: string,
    ) {
      super(
        reason === "expired"
          ? "Pairing request expired before it was approved."
          : reason === "refused"
            ? `Allternit refused this device: ${detail}`
            : "Pairing request was denied.",
      )
      this.name = "PairingEnded"
    }
  }

  const log = Log.create({ service: "pairing" })

  const REQUEST_TIMEOUT_MS = 10_000
  const DEFAULT_POLL_INTERVAL_MS = 2_000
  // Rotate the device token once it is within this many days of expiry.
  // RUNTIME_PAIRING.md suggests rotating 7 days early; the wider window gives
  // machines that run `gizzi serve` rarely more chances to rotate in time.
  export const ROTATE_WITHIN_DAYS = 30
  // runtime_pairing.rs rejects anything outside this list; send the defaults.
  const CAPABILITIES = [
    "runtime:connect",
    "runtime:execute",
    "runtime:files",
    "runtime:remote_control",
    "runtime:terminal",
    "providers:connect",
    "providers:use",
  ]

  export interface Stored {
    version: 1
    name: string
    runtimeType: string
    hostname: string
    platform: string
    publicKey: string
    publicKeyFingerprint: string
    privateKey: string // PKCS8 PEM
    runtimeId?: string
    userId?: string
    userEmail?: string
    organizationId?: string
    deviceToken?: string
    tokenExpiresAt?: string
    capabilities?: string[]
    pairedAt?: string
  }

  export interface Created {
    pairingId: string
    deviceCode: string
    userCode: string
    challenge: string
    verificationUrl: string
    expiresAt: string
    pollIntervalSeconds: number
  }

  export type ExchangeResult =
    | { status: "pending" }
    | { status: "denied" }
    | { status: "refused"; message: string }
    | { status: "expired" }
    | { status: "ratelimited"; retryAfterMs: number }
    | {
        status: "approved"
        session: {
          runtimeId: string
          userId: string
          userEmail: string
          organizationId?: string
          deviceToken: string
          expiresAt: string
          capabilities: string[]
        }
      }

  function filepath() {
    return path.join(Global.Path.data, "runtime-device.json")
  }

  function platformBase() {
    return Flag.GIZZI_PLATFORM_API_URL.replace(/\/+$/, "")
  }

  export async function load(): Promise<Stored | undefined> {
    const data = await Filesystem.readJson<Stored>(filepath()).catch(() => undefined)
    if (!data || typeof data.publicKey !== "string" || typeof data.privateKey !== "string") return undefined
    return data
  }

  async function save(stored: Stored) {
    // Atomic replace: write a temp sibling, force 0600 on it, then rename over
    // the target so a crash mid-write can never truncate or widen the
    // permissions of the credential file (writeFile's mode only applies at
    // creation time).
    const target = filepath()
    const temp = `${target}.${process.pid}.tmp`
    await Filesystem.writeJson(temp, stored)
    await fs.chmod(temp, 0o600)
    await fs.rename(temp, target)
  }

  export async function clear() {
    // Best-effort unpair; a leftover file only reuses a stale token.
    await Filesystem.remove(filepath()).catch(() => {})
  }

  // A stored token counts as usable until its exact expiry; the caller decides
  // whether to re-pair (serve falls through to the next credential instead).
  export function tokenUsable(stored: Stored | undefined): stored is Stored {
    if (!stored?.deviceToken || !stored.tokenExpiresAt) return false
    return Date.parse(stored.tokenExpiresAt) > Date.now()
  }

  // True when the stored token is still usable but close enough to expiry that
  // it should be rotated on the next opportunity (serve start / register tick).
  export function rotationDue(stored: Stored | undefined): stored is Stored {
    if (!tokenUsable(stored)) return false
    return Date.parse(stored.tokenExpiresAt!) - Date.now() <= ROTATE_WITHIN_DAYS * 86_400_000
  }

  export async function status() {
    const stored = await load()
    if (!stored) return { paired: false as const }
    return {
      paired: tokenUsable(stored),
      expired: !!stored.deviceToken && !tokenUsable(stored),
      stored,
    }
  }

  // Rotates the device token when it is within ROTATE_WITHIN_DAYS of expiry.
  // Best-effort: never throws, never clears the stored credentials — a failed
  // rotation leaves the old token in place (it may still be valid) and the
  // caller proceeds with it. Returns true only when a new token was stored.
  export async function rotateIfNeeded(): Promise<boolean> {
    const stored = await load()
    if (!rotationDue(stored)) return false
    if (!stored.runtimeId) {
      log.warn("device token is due for rotation but no runtime id is stored; run `gizzi pair --force` to re-pair")
      return false
    }
    log.info("device token expires soon; rotating", { runtimeId: stored.runtimeId, expiresAt: stored.tokenExpiresAt })
    try {
      const response = await fetch(`${platformBase()}/api/v1/runtime-devices/${stored.runtimeId}/rotate`, {
        method: "POST",
        headers: { authorization: `Bearer ${stored.deviceToken}` },
        signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
      })
      if (!response.ok) {
        const body = await response.text().catch(() => "")
        log.warn(
          "device token rotation failed; keeping the current token. If it stops working, run `gizzi pair --force` to re-pair",
          { status: response.status, body },
        )
        return false
      }
      const rotated = (await response.json()) as { deviceToken?: string; expiresAt?: string }
      if (typeof rotated.deviceToken !== "string" || typeof rotated.expiresAt !== "string") {
        log.warn("device token rotation returned an unexpected response; keeping the current token")
        return false
      }
      stored.deviceToken = rotated.deviceToken
      stored.tokenExpiresAt = rotated.expiresAt
      await save(stored)
      log.info("device token rotated", { runtimeId: stored.runtimeId, expiresAt: stored.tokenExpiresAt })
      return true
    } catch (err) {
      log.warn(
        "device token rotation failed; keeping the current token. If it stops working, run `gizzi pair --force` to re-pair",
        { error: err instanceof Error ? err.message : String(err) },
      )
      return false
    }
  }

  export function signatureMessage(pairingId: string, challenge: string) {
    return `allternit-runtime-pairing:${pairingId}:${challenge}`
  }

  function rawPublicKey(publicKey: ReturnType<typeof generateKeyPairSync>["publicKey"]) {
    // Ed25519 SPKI DER is a fixed 12-byte prefix followed by the 32 raw key
    // bytes; the platform expects exactly those 32 bytes, base64url-encoded.
    const der = publicKey.export({ format: "der", type: "spki" })
    return der.subarray(der.length - 32)
  }

  export function generateIdentity() {
    const { publicKey, privateKey } = generateKeyPairSync("ed25519")
    const raw = rawPublicKey(publicKey)
    return {
      publicKey: raw.toString("base64url"),
      publicKeyFingerprint: createHash("sha256").update(raw).digest("hex"),
      privateKey: privateKey.export({ format: "pem", type: "pkcs8" }).toString(),
    }
  }

  function signChallenge(identity: { privateKey: string }, pairingId: string, challenge: string) {
    const key = createPrivateKey(identity.privateKey)
    return sign(null, Buffer.from(signatureMessage(pairingId, challenge), "utf8"), key).toString("base64url")
  }

  async function request(pathname: string, body: unknown) {
    const response = await fetch(`${platformBase()}${pathname}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    })
    return response
  }

  export async function createPairing(stored: Stored): Promise<Created> {
    const response = await request("/api/v1/runtime-pairings", {
      name: stored.name,
      runtimeType: stored.runtimeType,
      hostname: stored.hostname,
      platform: stored.platform,
      version: Installation.VERSION,
      publicKey: stored.publicKey,
      capabilities: CAPABILITIES,
    })
    if (!response.ok) {
      const body = await response.text().catch(() => "")
      throw new Error(`pairing request failed (${response.status}): ${body}`)
    }
    const created = (await response.json()) as Created
    return { ...created, verificationUrl: approvalUrl(created.verificationUrl) }
  }

  /**
   * The approval page lives at ai.allternit.com/pair (the workspace, signed
   * in with the same account as Desktop). platform.allternit.com has no
   * /pair page, but the cloud API still builds links on it; a user who
   * opened one saw themselves signed in while nothing approved the code
   * and the CLI waited forever. Codes work on either host.
   */
  export function approvalUrl(url: string): string {
    try {
      const parsed = new URL(url)
      if (parsed.hostname === "platform.allternit.com" && parsed.pathname.replace(/\/$/, "") === "/pair") {
        parsed.hostname = "ai.allternit.com"
        return parsed.toString()
      }
    } catch {
      // not a URL; leave it
    }
    return url
  }

  export async function exchange(stored: Stored, pairing: Created): Promise<ExchangeResult> {
    const response = await request("/api/v1/runtime-pairings/exchange", {
      pairingId: pairing.pairingId,
      deviceCode: pairing.deviceCode,
      signature: signChallenge(stored, pairing.pairingId, pairing.challenge),
    })
    if (response.status === 428) return { status: "pending" }
    if (response.status === 410) return { status: "expired" }
    if (response.status === 403) {
      // 403 means "denied" only when the user clicked Deny. After an approval
      // the platform also answers 403 for its own limits ("Active runtime
      // limit reached …"); say that instead of "cancelled".
      const body = (await response.json().catch(() => undefined)) as
        | { error?: string; code?: string; message?: string }
        | undefined
      if (!body || body.error === "access_denied") return { status: "denied" }
      return { status: "refused", message: body.message || body.error || "the request was refused" }
    }
    if (response.status === 429) {
      // The cloud API rate-limits the exchange poll (and the approval page
      // polls the same bucket); honor retry_after instead of dying.
      const body = (await response.json().catch(() => undefined)) as { retry_after?: number } | undefined
      return { status: "ratelimited", retryAfterMs: Math.max(1, body?.retry_after ?? 15) * 1000 }
    }
    if (!response.ok) {
      const body = await response.text().catch(() => "")
      throw new Error(`pairing exchange failed (${response.status}): ${body}`)
    }
    const session = await response.json()
    return { status: "approved", session }
  }

  export interface PairOptions {
    name?: string
    runtimeType?: "desktop" | "vps"
    // Called once the pairing is created so the CLI can show code + URL.
    onCreated?: (pairing: Created) => void
    // Called between exchange polls (attempt is 1-based).
    onPoll?: (attempt: number) => void
  }

  // Runs the full device-code flow: create pairing, open the approval page,
  // poll the exchange endpoint until the user approves or the pairing expires.
  // Resolves with the persisted device record.
  /**
   * Approve in Allternit Desktop (allternit://pair?code=…) instead of the
   * browser. Opt-in only (GIZZI_PAIR_VIA=desktop): by default the CLI opens
   * ai.allternit.com/pair in the browser, the workspace page signed in with
   * the same account as Desktop.
   */
  export function desktopApproval(): boolean {
    return process.env.GIZZI_PAIR_VIA === "desktop"
  }

  export function desktopApprovalLink(userCode: string): string {
    return `allternit://pair?code=${encodeURIComponent(userCode)}`
  }

  /**
   * This machine's name as the platform should see it. On macOS
   * `os.hostname()` follows the network ("joes-MBP.hsd1.mn.comcast.net" on
   * one, "joes-MacBook-Pro.local" on another), so the platform took gizzi and
   * Desktop on one Mac for two machines. The LocalHostName doesn't change.
   */
  export function machineHostname(): string {
    if (process.platform === "darwin") {
      try {
        const name = execFileSync("scutil", ["--get", "LocalHostName"], { encoding: "utf8", timeout: 2000 }).trim()
        if (name) return name
      } catch {
        // fall back below
      }
    }
    return os.hostname()
  }

  export async function pair(opts: PairOptions = {}): Promise<Stored> {
    const existing = await load()
    const identity =
      existing && existing.publicKey && existing.privateKey
        ? {
            publicKey: existing.publicKey,
            publicKeyFingerprint: existing.publicKeyFingerprint,
            privateKey: existing.privateKey,
          }
        : generateIdentity()

    const stored: Stored = {
      version: 1,
      name: opts.name ?? existing?.name ?? os.hostname(),
      runtimeType: opts.runtimeType ?? existing?.runtimeType ?? "desktop",
      hostname: machineHostname(),
      platform: `${process.platform}-${process.arch}`,
      ...identity,
    }

    const pairing = await createPairing(stored)
    log.info("pairing created", { pairingId: pairing.pairingId, userCode: pairing.userCode })
    opts.onCreated?.(pairing)

    if (!process.env.GIZZI_PAIR_NO_BROWSER) {
      try {
        await open(desktopApproval() ? desktopApprovalLink(pairing.userCode) : pairing.verificationUrl)
      } catch (err) {
        log.warn("failed to open browser for pairing approval", {
          error: err instanceof Error ? err.message : String(err),
        })
      }
    }

    const deadline = Date.parse(pairing.expiresAt)
    const interval = Math.max(500, (pairing.pollIntervalSeconds || 2) * 1000) || DEFAULT_POLL_INTERVAL_MS
    let attempt = 0
    while (Date.now() < deadline) {
      attempt += 1
      opts.onPoll?.(attempt)
      const result = await exchange(stored, pairing)
      if (result.status === "pending") {
        await new Promise((resolve) => setTimeout(resolve, interval))
        continue
      }
      if (result.status === "ratelimited") {
        await new Promise((resolve) => setTimeout(resolve, result.retryAfterMs))
        continue
      }
      if (result.status === "expired") throw new PairingEnded("expired")
      if (result.status === "denied") throw new PairingEnded("denied")
      if (result.status === "refused") throw new PairingEnded("refused", result.message)
      stored.runtimeId = result.session.runtimeId
      stored.userId = result.session.userId
      stored.userEmail = result.session.userEmail
      stored.organizationId = result.session.organizationId
      stored.deviceToken = result.session.deviceToken
      stored.tokenExpiresAt = result.session.expiresAt
      stored.capabilities = result.session.capabilities
      stored.pairedAt = new Date().toISOString()
      await save(stored)
      log.info("runtime device paired", { runtimeId: stored.runtimeId, name: stored.name })
      return stored
    }
    throw new Error("Pairing request expired before it was approved. Run `gizzi pair` again.")
  }
}
