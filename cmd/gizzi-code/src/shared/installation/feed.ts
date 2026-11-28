/**
 * Gizzi release feed: fetch + Ed25519 verify + cache of the last check.
 *
 *   GET https://runtime.allternit.com/gizzi/<channel>/latest.json      channel = stable | latest
 *   GET https://runtime.allternit.com/gizzi/<channel>/latest.json.sig  Ed25519 over the exact bytes, base64
 *
 * Contract: handoffs/gizzi-overhaul-2026-11-27/PLAN.md section 3 "Release feed".
 * The signature check copies Desktop's runtime feed
 * (surfaces/allternit-desktop/src/main/runtime-package.ts verifySignature + runCheck),
 * with a Gizzi-only key (feed-key.ts).
 *
 * Fail closed: a client that cannot verify the feed does nothing and says why
 * (`reason`), which `gizzi doctor` prints. Binaries stay on GitHub Releases;
 * the signed feed carries each sha256, so the byte host needs no trust.
 *
 * This module has no side effects on import and takes every edge (fetch,
 * key, state dir, clock) as an option so tests drive it without a network.
 */
import crypto from "node:crypto"
import fs from "node:fs"
import path from "node:path"
import z from "zod/v4"
import { GlobalPaths } from "@/runtime/context/global/paths"
import { GIZZI_FEED_KEY_PLACEHOLDER, GIZZI_FEED_PUBLIC_KEY } from "./feed-key"

export namespace ReleaseFeed {
  export const DEFAULT_BASE = "https://runtime.allternit.com/gizzi"
  export const CHANNELS = ["stable", "latest"] as const
  export type Channel = (typeof CHANNELS)[number]
  /** Check on launch, then every 6 hours. */
  export const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000
  /** A feed file is small; a check that takes longer than this is abandoned. */
  export const FETCH_TIMEOUT_MS = 15_000
  /** latest.json and its .sig are uploaded one after the other; re-read a mismatched pair once after this. */
  export const PAIR_RETRY_MS = 3_000

  export const Artifact = z.object({
    url: z
      .string()
      .url()
      .refine((u) => u.startsWith("https://"), "artifact url must be https"),
    sha256: z.string().regex(/^[0-9a-f]{64}$/i),
    size: z.number().int().nonnegative(),
  })
  export type Artifact = z.infer<typeof Artifact>

  export const Feed = z.object({
    schema: z.literal(1),
    channel: z.enum(CHANNELS),
    version: z.string().min(1),
    publishedAt: z.string(),
    minVersion: z.string().min(1).optional(),
    paused: z.boolean().default(false),
    notes: z
      .object({
        url: z.string().optional(),
        highlights: z.array(z.string()).default([]),
      })
      .optional(),
    artifacts: z.record(z.string(), Artifact).default({}),
    packages: z
      .object({
        npm: z.string().optional(),
        brew: z.string().optional(),
        scoop: z.string().optional(),
        choco: z.string().optional(),
      })
      .partial()
      .default({}),
  })
  export type Feed = z.infer<typeof Feed>

  /** Why a check produced no usable feed. Stable strings: doctor and the UI print them. */
  export type FailReason =
    | "feed key not set"
    | "feed signature invalid"
    | "feed unreadable"
    | "feed unreachable"

  export type FetchResult =
    | { ok: true; feed: Feed; raw: Buffer }
    | { ok: false; reason: FailReason; detail?: string }

  export interface FetchOptions {
    channel?: Channel
    /** Feed root; defaults to env GIZZI_FEED_URL, then DEFAULT_BASE. */
    base?: string
    /** PEM public key; defaults to activeKey(). */
    publicKey?: string
    fetch?: typeof fetch
    timeoutMs?: number
    retryMs?: number
  }

  // ---------------------------------------------------------------------------
  // Key + signature
  // ---------------------------------------------------------------------------

  /** The key in use: env GIZZI_FEED_PUBLIC_KEY (tests, staging feeds) or the shipped constant. */
  export function activeKey(env: NodeJS.ProcessEnv = process.env): string {
    const override = env.GIZZI_FEED_PUBLIC_KEY
    if (override !== undefined) return override.replace(/\\n/g, "\n")
    return GIZZI_FEED_PUBLIC_KEY
  }

  /** True when the key is empty or still the placeholder marker: the updater must do nothing. */
  export function keyMissing(key: string): boolean {
    return !key.trim() || key.includes(GIZZI_FEED_KEY_PLACEHOLDER)
  }

  /** Ed25519 over the exact bytes, base64 signature (same rule as Desktop's runtime feed). */
  export function verifySignature(data: Buffer, signatureB64: string, publicKey: string): boolean {
    try {
      return crypto.verify(null, data, publicKey, Buffer.from(signatureB64.trim(), "base64"))
    } catch {
      return false
    }
  }

  // ---------------------------------------------------------------------------
  // Channel
  // ---------------------------------------------------------------------------

  export function isChannel(value: unknown): value is Channel {
    return typeof value === "string" && (CHANNELS as readonly string[]).includes(value)
  }

  /** env GIZZI_UPDATE_CHANNEL, then the saved choice, then `stable`. */
  export function channel(env: NodeJS.ProcessEnv = process.env, stateDir?: string): Channel {
    const fromEnv = env.GIZZI_UPDATE_CHANNEL?.trim().toLowerCase()
    if (isChannel(fromEnv)) return fromEnv
    const saved = readState(stateDir).channel
    if (isChannel(saved)) return saved
    return "stable"
  }

  export function feedBase(env: NodeJS.ProcessEnv = process.env): string {
    return (env.GIZZI_FEED_URL?.trim() || DEFAULT_BASE).replace(/\/+$/, "")
  }

  export function feedUrl(ch: Channel, base = feedBase()): string {
    return `${base.replace(/\/+$/, "")}/${ch}/latest.json`
  }

  // ---------------------------------------------------------------------------
  // Fetch + verify
  // ---------------------------------------------------------------------------

  /** Parse already-verified bytes. Exposed so the contract fixture (docs/contracts/gizzi-release-feed) can be checked. */
  export function parse(raw: Buffer | string): Feed {
    const text = typeof raw === "string" ? raw : raw.toString("utf8")
    return Feed.parse(JSON.parse(text))
  }

  export async function fetchFeed(opts: FetchOptions = {}): Promise<FetchResult> {
    const publicKey = opts.publicKey ?? activeKey()
    if (keyMissing(publicKey)) return { ok: false, reason: "feed key not set" }
    const fetchImpl = opts.fetch ?? fetch
    const ch = opts.channel ?? channel()
    const url = feedUrl(ch, opts.base ?? feedBase())
    const timeoutMs = opts.timeoutMs ?? FETCH_TIMEOUT_MS
    const retryMs = opts.retryMs ?? PAIR_RETRY_MS
    const small = (u: string) =>
      fetchImpl(u, { cache: "no-store", signal: AbortSignal.timeout(timeoutMs) } as RequestInit)

    let raw: Buffer | null = null
    try {
      // A check that lands between the two uploads sees a mismatched pair: read the pair again once.
      for (let attempt = 1; attempt <= 2 && !raw; attempt++) {
        if (attempt > 1) await new Promise((r) => setTimeout(r, retryMs))
        const res = await small(url)
        if (!res.ok) return { ok: false, reason: "feed unreachable", detail: `${url} ${res.status}` }
        const body = Buffer.from(await res.arrayBuffer())
        const sigRes = await small(`${url}.sig`)
        if (sigRes.ok && verifySignature(body, await sigRes.text(), publicKey)) raw = body
      }
    } catch (err) {
      return { ok: false, reason: "feed unreachable", detail: err instanceof Error ? err.message : String(err) }
    }
    if (!raw) return { ok: false, reason: "feed signature invalid" }
    try {
      const feed = parse(raw)
      // A signed feed for another channel served at this URL is refused, not followed.
      if (feed.channel !== ch) return { ok: false, reason: "feed unreadable", detail: `channel ${feed.channel} served for ${ch}` }
      return { ok: true, feed, raw }
    } catch (err) {
      return { ok: false, reason: "feed unreadable", detail: err instanceof Error ? err.message : String(err) }
    }
  }

  // ---------------------------------------------------------------------------
  // Cached state (last check, staged update, last update)
  // ---------------------------------------------------------------------------

  /** One file the swap moves into place. `from` is in the install dir, so the move is one rename. */
  export interface StagedFile {
    from: string
    to: string
    /** Hash of `from` when staged; re-checked before the swap. */
    sha256: string
  }

  export interface StagedUpdate {
    version: string
    /** Staging dir next to the installed binary (same filesystem). */
    dir: string
    /** The installed binary the update replaces (real path). */
    target: string
    /** The main binary first, then helpers shipped beside it (allternit-factory, allternit-files-mount). */
    files: StagedFile[]
    stagedAt: string
  }

  export interface State {
    channel?: Channel
    lastCheckAt?: string
    /** "ok" or a FailReason. */
    lastCheckResult?: string
    lastCheckDetail?: string
    latestVersion?: string
    minVersion?: string
    paused?: boolean
    highlights?: string[]
    notesUrl?: string
    staged?: StagedUpdate
    /** npm path: the version handed to `npm i -g` on exit, confirmed on the next launch. */
    pendingPackage?: { version: string; from: string; command: string; startedAt: string }
    lastUpdateAt?: string
    lastUpdateFrom?: string
    lastUpdateTo?: string
  }

  /** Lives in gizzi's state dir (~/.local/state/gizzi-code/update.json), not the user-edited gizzi.json. */
  export function statePath(stateDir = GlobalPaths.state): string {
    return path.join(stateDir, "update.json")
  }

  export function readState(stateDir?: string): State {
    try {
      const parsed = JSON.parse(fs.readFileSync(statePath(stateDir), "utf8"))
      return parsed && typeof parsed === "object" ? (parsed as State) : {}
    } catch {
      return {}
    }
  }

  /** Merge `patch` into the saved state (atomic write; a failed write never throws into the caller). */
  export function writeState(patch: Partial<State>, stateDir?: string): State {
    const next: State = { ...readState(stateDir), ...patch }
    for (const key of Object.keys(next) as (keyof State)[]) if (next[key] === undefined) delete next[key]
    try {
      const file = statePath(stateDir)
      fs.mkdirSync(path.dirname(file), { recursive: true })
      const tmp = `${file}.${process.pid}.tmp`
      fs.writeFileSync(tmp, JSON.stringify(next, null, 2))
      fs.renameSync(tmp, file)
    } catch {
      // state is a cache; losing one write only means the next launch checks again
    }
    return next
  }

  /** Save the user's channel choice (`gizzi update --channel latest` in step 18). */
  export function setChannel(ch: Channel, stateDir?: string): void {
    writeState({ channel: ch }, stateDir)
  }

  /** Record a check's outcome so doctor can show "last check" without a network call. */
  export function recordCheck(result: FetchResult, now: Date, stateDir?: string): State {
    if (result.ok === false) {
      return writeState(
        { lastCheckAt: now.toISOString(), lastCheckResult: result.reason, lastCheckDetail: result.detail },
        stateDir,
      )
    }
    const { feed } = result
    return writeState(
      {
        lastCheckAt: now.toISOString(),
        lastCheckResult: "ok",
        lastCheckDetail: undefined,
        latestVersion: feed.version,
        minVersion: feed.minVersion,
        paused: feed.paused,
        highlights: feed.notes?.highlights,
        notesUrl: feed.notes?.url,
      },
      stateDir,
    )
  }
}
