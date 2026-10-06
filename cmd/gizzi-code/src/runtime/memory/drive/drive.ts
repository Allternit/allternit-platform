/**
 * MemoryDrive — gizzi's entry point to the user's Memory Drive(s).
 *
 * - personal drive: always available. Signed out it is a local-only repo under
 *   gizzi's data dir (`local` account); signed in it is a checkout of the
 *   account's drive (clone_url from GET /memory/drive/info, write token minted
 *   via POST /memory/drive/tokens). The first sign-in merges the signed-out
 *   history up (merge commit, never reset/force); the local drive is then
 *   claimed by that account, so another account never receives it.
 * - mounts (GET /memory/drive/mounts): project/team/bot/swarm drives, each in
 *   its own checkout, read-only unless the mount grants write.
 * Writes target personal unless a drive ref is given.
 */
import { existsSync } from "fs"
import { readFile, writeFile } from "fs/promises"
import os from "os"
import path from "path"
import { Log } from "@/shared/util/log"
import { DriveCheckout, type DriveRemote, type DriveStatus, type SearchHit, type WriteResult } from "./checkout"
import {
  boundedEntrypoint,
  collectEntries,
  deriveEntryId,
  ENTRYPOINT,
  today,
  validatePath,
  validateSource,
  type Entry,
  type Operation,
} from "./format"
import {
  driveAccountSync,
  driveCheckoutPath,
  localDriveCheckoutPath,
  LOCAL_ACCOUNT,
  memoryDriveEnabled,
  serverDreamingActiveSync as serverDreamingActiveSyncImpl,
  type DriveAccount,
} from "./paths"
import type { DriveOverview } from "./report"
import {
  DrivePlatform,
  readAccountStore,
  updateAccountStore,
  type Dream,
  type DriveMount,
} from "./platform"

const log = Log.create({ service: "memory-drive" })

export type LegacyMemoryType = "user" | "feedback" | "project" | "reference"
export const MEMORY_TYPES = ["fact", "preference", "event", "procedure", "entity", "relationship", "task_state"] as const
export type DriveMemoryType = (typeof MEMORY_TYPES)[number]

export interface DriveContextBlock {
  ref: string
  name: string
  dir: string
  readOnly: boolean
  entrypoint: string
  status?: DriveStatus
}

export interface RememberInput {
  text: string
  /** Topic path (without or with .md), e.g. `preferences` or `projects/gizzi`. */
  topic?: string
  type?: LegacyMemoryType | DriveMemoryType
  /** Update this stable id instead of creating a new entry. */
  id?: string
  sessionId?: string
  ref?: string
  /** Project directory, used for the default `projects/<name>` topic. */
  projectDir?: string
}

function author() {
  const host = os.hostname().replace(/[<>\n\r]/g, "").slice(0, 60) || "this computer"
  return { name: `gizzi on ${host}`, email: "gizzi@allternit.local" }
}

/** `gizzi:session/<id>` — the contract source for a local gizzi session. */
export function sessionSource(sessionId?: string): string {
  const id = (sessionId ?? "").replace(/[^A-Za-z0-9_.:-]/g, "").slice(0, 120)
  return id ? `gizzi:session/${id}` : "gizzi:session/unknown"
}

/** Legacy memdir type / free-form type → contract memory_type. */
export function memoryTypeFor(type?: string): DriveMemoryType {
  switch (type) {
    case "feedback":
      return "preference"
    case "project":
      return "task_state"
    case "user":
    case "reference":
    case undefined:
    case "":
      return "fact"
    default:
      return (MEMORY_TYPES as readonly string[]).includes(type) ? (type as DriveMemoryType) : "fact"
  }
}

/** Safe topic segment: lowercase letters, digits, `-`, `_`. */
export function topicSlug(value: string): string {
  const slug = value
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^[-_]+|[-_]+$/g, "")
    .slice(0, 60)
  if (!slug) return "notes"
  // Avoid names the format reserves for transcripts/logs (session, log, chat…).
  for (const candidate of [slug, `notes-${slug}`]) {
    try {
      validatePath(`${candidate}.md`)
      return candidate
    } catch {
      continue
    }
  }
  return "notes"
}

/** Where a remembered fact goes: explicit topic, else one topic file per type. */
export function topicPath(topic?: string, type?: string, projectDir?: string): string {
  if (topic && topic.trim()) {
    const parts = topic.trim().replace(/\.md$/i, "").split("/").filter(Boolean).map(topicSlug)
    const p = `${parts.join("/") || "notes"}.md`
    validatePath(p)
    return p
  }
  switch (type) {
    case "user":
      return "user.md"
    case "feedback":
    case "preference":
      return "preferences.md"
    case "project":
    case "task_state":
      return projectDir ? `projects/${topicSlug(path.basename(projectDir))}.md` : "projects.md"
    case "reference":
      return "references.md"
    default:
      return "notes.md"
  }
}

export namespace MemoryDrive {
  export const enabled = memoryDriveEnabled

  const checkouts = new Map<string, { checkout: DriveCheckout; at: number; remote: boolean }>()
  /** Signed in but the API was unreachable: retry remote discovery after this long. */
  const REMOTE_RETRY_MS = 60_000
  const prepared = new Map<string, Promise<void>>()

  async function remoteFor(account: DriveAccount, ref: string, access: "read" | "write"): Promise<DriveRemote | undefined> {
    if (!account.signedIn) return undefined
    const store = await readAccountStore(account)
    let saved = store.drives[ref]
    if (!saved?.clone_url) {
      try {
        const info = await DrivePlatform.info(ref)
        await updateAccountStore(account, (s) => {
          s.drives[ref] = { ...s.drives[ref], clone_url: info.clone_url, branch: info.branch, name: info.name, brain_id: info.brain_id }
        })
        saved = { ...saved, clone_url: info.clone_url, branch: info.branch, name: info.name }
      } catch (error) {
        log.info("memory drive info unavailable; using the local checkout", { ref, error: String(error) })
        return undefined
      }
    }
    const mint = async () => {
      const minted = await DrivePlatform.mintToken(ref, access)
      await updateAccountStore(account, (s) => {
        s.drives[ref] = { ...s.drives[ref], token: minted.token, token_id: minted.id, access: minted.access, clone_url: minted.clone_url || s.drives[ref]?.clone_url }
      })
      return minted.token
    }
    return {
      url: saved!.clone_url!,
      branch: saved!.branch || "main",
      readOnly: access === "read",
      token: async () => {
        const current = (await readAccountStore(account)).drives[ref]
        if (current?.token && (current.access === access || current.access === "write")) return current.token
        return mint()
      },
      refresh: mint,
    }
  }

  function accessFor(store: { mounts?: DriveMount[] }, ref: string): "read" | "write" {
    if (ref === "personal") return "write"
    return store.mounts?.find((m) => m.ref === ref)?.access ?? "read"
  }

  /** The checkout for a drive of the current account (created lazily). */
  export async function open(ref = "personal", options: { defaultSource?: string } = {}): Promise<DriveCheckout> {
    const account = driveAccountSync()
    const dir = driveCheckoutPath(ref, account)
    const existing = checkouts.get(dir)
    if (existing && (existing.remote || !account.signedIn || Date.now() - existing.at < REMOTE_RETRY_MS)) return existing.checkout
    const store = await readAccountStore(account)
    const remote = await remoteFor(account, ref, accessFor(store, ref))
    const checkout = new DriveCheckout({
      dir,
      remote,
      author: author(),
      quarantineDir: `${dir}.rejected`,
      defaultSource: options.defaultSource,
    })
    checkouts.set(dir, { checkout, at: Date.now(), remote: !!remote })
    return checkout
  }

  /** Path of the personal checkout (sync; for prompts and memdir resolution). */
  export function personalDir(): string {
    return driveCheckoutPath("personal")
  }

  /**
   * Session start: create/open the personal checkout, merge the signed-out
   * drive on first sign-in, sync, and refresh mounts + Dream settings.
   * Once per process per account; failures are recorded, never thrown.
   */
  export function prepare(): Promise<void> {
    const account = driveAccountSync()
    let p = prepared.get(account.key)
    if (!p) {
      p = prepareAccount(account).catch((error) => {
        log.warn("memory drive prepare failed", { error: String(error) })
      })
      prepared.set(account.key, p)
    }
    return p
  }

  async function prepareAccount(account: DriveAccount): Promise<void> {
    const personal = await open("personal")
    await personal.ensure()
    if (!account.signedIn) return
    await mergeSignedOut(account, personal).catch((error) =>
      log.warn("merging signed-out memory failed; will retry next session", { error: String(error) }),
    )
    await personal.sync()
    const mounted = await refreshMounts(account).catch(() => [] as DriveMount[])
    await refreshSettings().catch(() => {})
    for (const mount of mounted.filter((m) => m.ref !== "personal").slice(0, MAX_MOUNTS)) {
      await (await open(mount.ref)).sync().catch((error) => log.info("mounted drive sync failed", { ref: mount.ref, error: String(error) }))
    }
  }

  const MAX_MOUNTS = 8

  const CLAIM_FILE = "allternit-claimed-by"

  /** First sign-in: merge the `local` drive into this account, once; never into a second account. */
  async function mergeSignedOut(account: DriveAccount, personal: DriveCheckout): Promise<void> {
    const localDir = localDriveCheckoutPath()
    if (!existsSync(path.join(localDir, ".git")) || !personal.hasRemote) return
    const claimPath = path.join(localDir, ".git", CLAIM_FILE)
    const claimedBy = await readFile(claimPath, "utf8").then((t) => t.trim()).catch(() => "")
    if (claimedBy && claimedBy !== account.key) return
    const local = new DriveCheckout({ dir: localDir, author: author() })
    const localHead = await local.head()
    const store = await readAccountStore(account)
    if (!localHead || store.mergedLocal === localHead) return
    if (!claimedBy) await writeFile(claimPath, account.key)
    const merged = await personal.mergeFrom(localDir)
    if (merged.merged || merged.reason === "already merged") {
      await updateAccountStore(account, (s) => {
        s.mergedLocal = localHead
      })
    }
  }

  export async function refreshMounts(account: DriveAccount = driveAccountSync()): Promise<DriveMount[]> {
    if (!account.signedIn) return []
    const mounts = await DrivePlatform.mounts()
    await updateAccountStore(account, (s) => {
      s.mounts = mounts
      s.mountsAt = new Date().toISOString()
    })
    return mounts
  }

  /** Mounted drives other than personal (cached list; refreshed at session start). */
  export async function mounts(): Promise<DriveMount[]> {
    const account = driveAccountSync()
    if (!account.signedIn) return []
    const store = await readAccountStore(account)
    return (store.mounts ?? []).filter((m) => m.ref !== "personal")
  }

  export async function refreshSettings(ref = "personal"): Promise<boolean> {
    const account = driveAccountSync()
    if (!account.signedIn) return false
    const settings = await DrivePlatform.settings(ref)
    await updateAccountStore(account, (s) => {
      s.settings = { ...s.settings, [ref]: { dreaming_enabled: !!settings.dreaming_enabled, at: new Date().toISOString() } }
    })
    return !!settings.dreaming_enabled
  }

  /** True when the server's nightly Dream owns the personal drive (local autoDream stays off). */
  export const serverDreamingActiveSync = serverDreamingActiveSyncImpl

  // ── writes ────────────────────────────────────────────────────────────────

  export async function write(operations: Operation[], options: { ref?: string; message: string; sessionId?: string }): Promise<WriteResult> {
    const checkout = await open(options.ref ?? "personal")
    contextCache = undefined
    return checkout.write(operations, { message: options.message, source: sessionSource(options.sessionId) })
  }

  /** Save (or update by id) one fact as a spec bullet. */
  export async function remember(input: RememberInput): Promise<{ entry: Entry; path: string; ref: string; result: WriteResult }> {
    const ref = input.ref ?? "personal"
    const text = input.text.replace(/\s+/g, " ").trim()
    const source = sessionSource(input.sessionId)
    validateSource(source)
    const added = today()
    const checkout = await open(ref)
    await checkout.ensure()
    let target = input.topic || !input.id ? topicPath(input.topic, input.type, input.projectDir) : undefined
    let existing: Entry | undefined
    if (input.id) {
      const { files } = await checkout.workingFiles()
      const found = safeCollect(files).get(input.id)
      if (found) {
        existing = found.entry
        target ??= found.path
      }
      target ??= topicPath(undefined, input.type, input.projectDir)
    }
    const id = input.id ?? deriveEntryId(text, source, added)
    const entry: Entry = {
      id,
      text,
      source,
      added,
      metadata: {
        ...(existing?.metadata ?? {}),
        memory_type: memoryTypeFor(input.type ?? existing?.metadata.memory_type),
        agent: "gizzi",
      },
    }
    contextCache = undefined
    const result = await checkout.write([{ UpsertEntry: { path: target!, entry } }], {
      message: existing ? `Update memory: ${summary(text)}` : `Remember: ${summary(text)}`,
      source,
    })
    return { entry, path: target!, ref, result }
  }

  export async function forget(id: string, options: { ref?: string; sessionId?: string } = {}): Promise<{ found: boolean; result?: WriteResult }> {
    const checkout = await open(options.ref ?? "personal")
    await checkout.ensure()
    const { files } = await checkout.workingFiles()
    const found = safeCollect(files).get(id)
    if (!found) return { found: false }
    contextCache = undefined
    const result = await checkout.write([{ DeleteEntry: { id } }], {
      message: `Forget: ${summary(found.entry.text)}`,
      source: sessionSource(options.sessionId),
    })
    return { found: true, result }
  }

  /** Fold edits made with file tools / an editor into a commit and sync. */
  export async function commitWorkingTree(options: { ref?: string; sessionId?: string; message?: string } = {}) {
    const checkout = await open(options.ref ?? "personal")
    contextCache = undefined
    return checkout.commitWorkingTree({ message: options.message, source: sessionSource(options.sessionId) })
  }

  export async function sync(ref = "personal"): Promise<DriveStatus> {
    return (await open(ref)).sync()
  }

  export async function status(ref = "personal"): Promise<DriveStatus> {
    return (await open(ref)).status()
  }

  /** Status, files and recent history of one drive (TUI /memory, `gizzi memory status`). */
  export async function overview(ref = "personal", historyLimit = 10): Promise<DriveOverview> {
    const account = driveAccountSync()
    const checkout = await open(ref)
    await checkout.ensure()
    const mount = ref === "personal" ? undefined : (await mounts()).find((m) => m.ref === ref)
    return {
      ref,
      name: ref === "personal" ? (account.signedIn ? "Personal" : "Personal (local, signed out)") : (mount?.name ?? ref),
      signedIn: account.signedIn,
      status: await checkout.status(),
      files: await checkout.listFiles(),
      history: await checkout.history(historyLimit),
    }
  }

  // ── reads ─────────────────────────────────────────────────────────────────

  /** personal + every mount, each with its MEMORY.md (bounded) and sync status. */
  export async function contextBlocks(options: { includeMounts?: boolean; maxMountLines?: number } = {}): Promise<DriveContextBlock[]> {
    const blocks: DriveContextBlock[] = []
    const account = driveAccountSync()
    const personal = await open("personal")
    await personal.ensure().catch(() => {})
    blocks.push({
      ref: "personal",
      name: account.signedIn ? "Personal" : "Personal (local, signed out)",
      dir: personal.dir,
      readOnly: false,
      entrypoint: await readEntrypoint(personal.dir),
      status: await personal.status().catch(() => undefined),
    })
    if (options.includeMounts === false) return blocks
    for (const mount of (await mounts()).slice(0, MAX_MOUNTS)) {
      // Mount checkouts are cloned/synced by prepare(); only show ones present.
      const dir = driveCheckoutPath(mount.ref, account)
      if (!existsSync(path.join(dir, ".git"))) continue
      const checkout = await open(mount.ref)
      blocks.push({
        ref: mount.ref,
        name: mount.name,
        dir: checkout.dir,
        readOnly: mount.access !== "write",
        entrypoint: await readEntrypoint(checkout.dir, options.maxMountLines ?? 80),
        status: await checkout.status().catch(() => undefined),
      })
    }
    return blocks
  }

  async function readEntrypoint(dir: string, maxLines = 200): Promise<string> {
    const raw = await readFile(path.join(dir, ENTRYPOINT), "utf8").catch(() => "")
    return raw ? boundedEntrypoint(raw, maxLines) : ""
  }

  /** Search every drive's files (personal first). */
  export async function search(query: string, limit = 50): Promise<(SearchHit & { ref: string; dir: string })[]> {
    const out: (SearchHit & { ref: string; dir: string })[] = []
    const refs = ["personal", ...(await mounts()).map((m) => m.ref)]
    for (const ref of refs) {
      const checkout = await open(ref)
      if (!existsSync(path.join(checkout.dir, ".git"))) continue
      for (const hit of await checkout.search(query, limit - out.length)) out.push({ ...hit, ref, dir: checkout.dir })
      if (out.length >= limit) break
    }
    return out
  }

  /**
   * The system-prompt section for a session: each drive's MEMORY.md with its
   * checkout path, how to read topic files and how to save. Waits a bounded
   * time for session-start sync; never blocks a session on the network.
   */
  export async function sessionContext(options: { waitMs?: number; writeTool?: string; saveInstructions?: string; omitPersonalEntrypoint?: boolean } = {}): Promise<string> {
    if (!enabled()) return ""
    await Promise.race([prepare(), new Promise((r) => setTimeout(r, options.waitMs ?? 4_000))])
    // System prompts are rebuilt every turn; reuse the snapshot briefly.
    const key = driveAccountSync().key
    let blocks: DriveContextBlock[]
    if (contextCache && contextCache.key === key && Date.now() - contextCache.at < CONTEXT_TTL_MS) blocks = contextCache.blocks
    else {
      blocks = await contextBlocks().catch(() => [] as DriveContextBlock[])
      contextCache = { key, at: Date.now(), blocks }
    }
    if (blocks.length === 0) return ""
    // Harnesses that already inject the personal MEMORY.md (the TUI's AutoMem file) skip the copy.
    if (options.omitPersonalEntrypoint) {
      blocks = blocks.map((b) => (b.ref === "personal" ? { ...b, entrypoint: "(Its MEMORY.md is loaded with your other memory files.)" } : b))
    }
    return renderContext(blocks, options.saveInstructions ?? toolSaveInstructions(options.writeTool ?? "memory_write"))
  }

  const CONTEXT_TTL_MS = 15_000
  let contextCache: { key: string; at: number; blocks: DriveContextBlock[] } | undefined

  export function toolSaveInstructions(writeTool: string): string {
    return `Save, update or delete memories with the \`${writeTool}\` tool. It adds the source, date and id, regenerates the index and syncs; do not edit MEMORY.md's index by hand. Never store credentials, transcripts or raw logs.`
  }

  /** For harnesses whose agent edits files directly (the interactive TUI). */
  export function fileSaveInstructions(): string {
    return [
      "To save a memory, add ONE line `- <fact>` to the right topic file in the personal checkout with your file edit/write tools (e.g. preferences.md, user.md, projects/<name>.md, references.md; create the file with a `# <Topic>` heading if it is new). gizzi adds the source, date and id, regenerates MEMORY.md's index, commits and syncs at the end of the turn.",
      "To change a memory, edit its line's text (keep the `[...]` metadata); to forget one, delete its line. Do not write under ## Index in MEMORY.md, do not use frontmatter, and keep one fact per line.",
      "Never store credentials, transcripts or raw logs: lines that look like secrets are refused and moved aside. twin/ and cowork/ folders are managed by Allternit and read-only.",
    ].join("\n")
  }

  export function renderContext(blocks: DriveContextBlock[], saveInstructions: string): string {
    const lines: string[] = [
      "# Memory Drive",
      "",
      "Your long-term memory is a git-backed Memory Drive in the Agent Memory Repo format: MEMORY.md is a short index, and each topic file holds one-line facts written as `- <fact> [source: <where it came from>; added: YYYY-MM-DD; id: <id>]`.",
      `Topic files listed under "## Index" as [[topic]] live at <checkout>/<topic>.md. Read them with your normal file tools when they are relevant to the task; they are not loaded automatically.`,
      saveInstructions,
      "",
    ]
    for (const block of blocks) {
      const access = block.readOnly ? "read-only" : "writable"
      lines.push(`## ${block.ref === "personal" ? block.name : `${block.name} (${block.ref})`} — ${access}`, "", `Checkout: \`${block.dir}\``)
      const status = block.status
      if (status?.pending) lines.push(`Sync: ${status.ahead} local change(s) not yet on the server${status.lastError ? ` — ${status.lastError}` : ""}.`)
      else if (status?.lastError) lines.push(`Sync problem: ${status.lastError}`)
      if (status?.rejected) lines.push(`Not saved: ${status.rejected}`)
      lines.push("", block.entrypoint ? block.entrypoint : "(MEMORY.md is empty — nothing saved yet.)", "")
    }
    return lines.join("\n").trimEnd()
  }

  // ── Dreaming (server-side, Phase 2) ───────────────────────────────────────

  export function dreams(ref = "personal", limit = 20): Promise<Dream[]> {
    return DrivePlatform.dreams(ref, limit)
  }

  export async function undoDream(id: string, ref = "personal") {
    const result = await DrivePlatform.undoDream(id, ref)
    // Pull the revert commit into the checkout.
    await (await open(ref)).sync().catch(() => {})
    return result
  }

  /** Tests: forget cached checkouts and prepare state. */
  export function resetForTests(): void {
    checkouts.clear()
    prepared.clear()
    contextCache = undefined
  }

  export const LOCAL = LOCAL_ACCOUNT
}

function summary(text: string): string {
  const s = text.replace(/[<>]/g, "").trim()
  return s.length > 72 ? `${s.slice(0, 69)}…` : s
}

function safeCollect(files: Record<string, string>) {
  try {
    return collectEntries(files)
  } catch {
    // A malformed hand edit elsewhere must not block finding an entry by id.
    const out = new Map<string, ReturnType<typeof collectEntries> extends Map<string, infer V> ? V : never>()
    for (const p of Object.keys(files)) {
      try {
        for (const [id, v] of collectEntries({ [p]: files[p]! })) if (!out.has(id)) out.set(id, v)
      } catch {
        continue
      }
    }
    return out
  }
}
