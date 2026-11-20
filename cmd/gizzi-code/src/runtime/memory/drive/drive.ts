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
  CONFLICT_KEY,
  conflictEntries,
  deriveEntryId,
  ENTRYPOINT,
  entryVersion,
  EntryVersionError,
  needsLookLine,
  readEntries,
  renderEntry,
  today,
  validatePath,
  validateSource,
  type DriveFiles,
  type Entry,
  type LocatedEntry,
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
import { sleepEnabled } from "../sleep/client"
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
  /** The version the caller last read (memory_list / memory_recall). A different current version refuses the write. */
  ifVersion?: string
  /** Refuse to change an existing entry when no `ifVersion` is given (the memory_write tool sets this). */
  requireVersion?: boolean
}

/** append adds text to the end of an entry; replace swaps `old` (must occur exactly once) for `new`. */
export type EditInput = {
  id: string
  ifVersion?: string
  requireVersion?: boolean
  sessionId?: string
  ref?: string
} & ({ action: "append"; text: string } | { action: "replace"; old: string; new: string })

export interface EditOutcome {
  entry: Entry
  path: string
  ref: string
  /** Version of the entry's line after the write (pass it as if_version next time). */
  version?: string
  result: WriteResult
  /** Another device changed the same entry at the same time: our text was kept under this sibling id. */
  conflict?: string
}

/** No entry with that id in the drive. */
export class EntryMissingError extends Error {
  constructor(
    readonly id: string,
    readonly ref: string,
  ) {
    super(`No memory with id ${id} in ${ref}. Find it with memory_list or memory_recall.`)
    this.name = "EntryMissingError"
  }
}

/** `replace` found `old` zero times or more than once. */
export class ReplaceMatchError extends Error {
  constructor(readonly matches: number) {
    super(
      matches === 0
        ? "The old text does not appear in that memory; copy it exactly from memory_list."
        : `The old text appears ${matches} times in that memory; include more words so it matches once.`,
    )
    this.name = "ReplaceMatchError"
  }
}

export interface ListedEntry {
  id: string
  version: string
  text: string
  added: string
  /** Set on a sibling that holds one side of a conflicting edit. */
  conflictOf?: string
}

export interface TopicListing {
  ref: string
  topics: { path: string; count: number; needsLook: number }[]
  needsLook: ListedEntry[]
}

export interface EntryListing {
  ref: string
  path: string
  entries: ListedEntry[]
  total: number
  /** Pass as `after` for the next page. */
  next?: string
}

function listed(e: LocatedEntry): ListedEntry {
  const conflictOf = e.entry.metadata[CONFLICT_KEY]
  return { id: e.entry.id, version: entryVersion(e.line), text: e.entry.text, added: e.entry.added, ...(conflictOf ? { conflictOf } : {}) }
}

/** Refuse when the caller's version is not the entry's current one (or none was given and one is required). */
function checkVersion(found: LocatedEntry, ifVersion: string | undefined, required: boolean): void {
  const version = entryVersion(found.line)
  const given = ifVersion?.trim()
  if (given ? given !== version : required) {
    throw new EntryVersionError(found.entry.id, found.entry.text, version, given ? "stale" : "missing")
  }
}

function countMatches(haystack: string, needle: string): number {
  let count = 0
  for (let at = haystack.indexOf(needle); at >= 0; at = haystack.indexOf(needle, at + needle.length)) count++
  return count
}

const oneLine = (text: string) => text.replace(/\s+/g, " ").trim()

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
    // Prime the Sleep status (and its cached bit for the sync autoDream gate).
    await sleepEnabled().catch(() => false)
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
      // Merge: the Sleep status bit (sleep_enabled) lives in the same entry.
      s.settings = { ...s.settings, [ref]: { ...s.settings?.[ref], dreaming_enabled: !!settings.dreaming_enabled, at: new Date().toISOString() } }
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
  /**
   * Save (or update by id) one fact as a spec bullet. Updating an existing id
   * checks `ifVersion` against the latest head inside the write lock; saving
   * over an entry settles a conflict, so `conflict_of` is dropped.
   */
  export async function remember(input: RememberInput): Promise<{ entry: Entry; path: string; ref: string; result: WriteResult; version?: string; conflict?: string }> {
    const ref = input.ref ?? "personal"
    const text = oneLine(input.text)
    const source = sessionSource(input.sessionId)
    validateSource(source)
    const added = today()
    const checkout = await open(ref)
    await checkout.ensure()
    const id = input.id ?? deriveEntryId(text, source, added)
    let entry!: Entry
    let target!: string
    let line = ""
    contextCache = undefined
    const result = await checkout.write(
      (files) => {
        const existing = input.id ? safeCollect(files).get(input.id) : undefined
        if (existing) checkVersion(existing, input.ifVersion, !!input.requireVersion)
        target = input.topic || !existing ? topicPath(input.topic, input.type, input.projectDir) : existing.path
        const kept = { ...(existing?.entry.metadata ?? {}) }
        delete kept[CONFLICT_KEY]
        entry = {
          id,
          text,
          source,
          added,
          metadata: { ...kept, memory_type: memoryTypeFor(input.type ?? existing?.entry.metadata.memory_type), agent: "gizzi" },
        }
        line = renderEntry(entry)
        return {
          operations: [{ UpsertEntry: { path: target, entry } }],
          message: existing ? `Update memory: ${summary(text)}` : `Remember: ${summary(text)}`,
        }
      },
      { message: `Remember: ${summary(text)}`, source },
    )
    return { entry, path: target, ref, result, ...(await afterWrite(checkout, id, line)) }
  }

  export async function forget(
    id: string,
    options: { ref?: string; sessionId?: string; ifVersion?: string; requireVersion?: boolean } = {},
  ): Promise<{ found: boolean; result?: WriteResult }> {
    const ref = options.ref ?? "personal"
    const checkout = await open(ref)
    await checkout.ensure()
    contextCache = undefined
    try {
      const result = await checkout.write(
        (files) => {
          const found = safeCollect(files).get(id)
          if (!found) throw new EntryMissingError(id, ref)
          checkVersion(found, options.ifVersion, !!options.requireVersion)
          return { operations: [{ DeleteEntry: { id } }], message: `Forget: ${summary(found.entry.text)}` }
        },
        { message: `Forget: ${id}`, source: sessionSource(options.sessionId) },
      )
      return { found: true, result }
    } catch (error) {
      if (error instanceof EntryMissingError) return { found: false }
      throw error
    }
  }

  /**
   * Partial edit of one entry: `append` adds text exactly as given (start it
   * with a space to separate words); `replace` swaps `old`, which must occur
   * exactly once, for `new`. The secret gate runs on the resulting line, so
   * fragments that only spell a credential together are refused.
   */
  export async function edit(input: EditInput): Promise<EditOutcome> {
    const ref = input.ref ?? "personal"
    const source = sessionSource(input.sessionId)
    validateSource(source)
    const checkout = await open(ref)
    await checkout.ensure()
    let entry!: Entry
    let target!: string
    let line = ""
    contextCache = undefined
    const result = await checkout.write(
      (files) => {
        const found = safeCollect(files).get(input.id)
        if (!found) throw new EntryMissingError(input.id, ref)
        checkVersion(found, input.ifVersion, !!input.requireVersion)
        let text: string
        if (input.action === "append") {
          if (!input.text) throw new Error("Pass the text to add.")
          text = oneLine(found.entry.text + input.text)
        } else {
          if (!input.old) throw new Error("Pass the exact old text to replace.")
          const matches = countMatches(found.entry.text, input.old)
          if (matches !== 1) throw new ReplaceMatchError(matches)
          const at = found.entry.text.indexOf(input.old)
          text = oneLine(found.entry.text.slice(0, at) + input.new + found.entry.text.slice(at + input.old.length))
        }
        if (!text) throw new Error("That edit would leave the memory empty; delete it instead.")
        target = found.path
        entry = { id: input.id, text, source, added: today(), metadata: { ...found.entry.metadata, agent: "gizzi" } }
        line = renderEntry(entry)
        return { operations: [{ UpsertEntry: { path: target, entry } }], message: `Update memory: ${summary(text)}` }
      },
      { message: `Update memory: ${input.id}`, source },
    )
    return { entry, path: target, ref, result, ...(await afterWrite(checkout, input.id, line)) }
  }

  /**
   * The entry's version as it stands after a write, and whether a concurrent
   * edit from another device turned our write into a `conflict_of` sibling.
   */
  async function afterWrite(checkout: DriveCheckout, id: string, line: string): Promise<{ version?: string; conflict?: string }> {
    const { files } = await checkout.workingFiles()
    const now = safeCollect(files).get(id)
    if (!now || now.line === line) return { version: now ? entryVersion(now.line) : undefined }
    const sibling = conflictEntries(files).find((e) => e.entry.metadata[CONFLICT_KEY] === id && e.entry.text === parseText(line))
    return { version: entryVersion(now.line), conflict: sibling?.entry.id }
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

  /** Resolve a drive given as a ref (`project:<id>`) or a mounted drive's name; personal when empty. */
  export async function resolveRef(drive?: string): Promise<string> {
    const wanted = drive?.trim()
    if (!wanted || wanted.toLowerCase() === "personal") return "personal"
    const mounted = await mounts()
    const match = mounted.find((m) => m.ref === wanted) ?? mounted.find((m) => m.name.toLowerCase() === wanted.toLowerCase())
    if (!match) {
      const names = mounted.map((m) => `${m.name} (${m.ref})`).join(", ")
      throw new Error(`No mounted drive named ${wanted}.${names ? ` Mounted: ${names}.` : " Only the personal drive is available."}`)
    }
    return match.ref
  }

  async function readFiles(ref: string): Promise<DriveFiles> {
    const checkout = await open(ref)
    if (ref === "personal") await checkout.ensure()
    else if (!existsSync(path.join(checkout.dir, ".git"))) return {}
    return (await checkout.workingFiles()).files
  }

  /** Topic files with their entry counts, plus every entry that needs a look. */
  export async function listTopics(ref = "personal"): Promise<TopicListing> {
    const entries = readEntries(await readFiles(ref))
    const byPath = new Map<string, { count: number; needsLook: number }>()
    for (const e of entries) {
      const row = byPath.get(e.path) ?? { count: 0, needsLook: 0 }
      row.count++
      if (e.entry.metadata[CONFLICT_KEY]) row.needsLook++
      byPath.set(e.path, row)
    }
    return {
      ref,
      topics: [...byPath].map(([p, row]) => ({ path: p, ...row })),
      needsLook: entries.filter((e) => e.entry.metadata[CONFLICT_KEY]).map(listed),
    }
  }

  /** One topic's entries in file order, `limit` per page after the entry id `after`. */
  export async function listEntries(ref: string, topic: string, options: { after?: string; limit?: number } = {}): Promise<EntryListing> {
    const files = await readFiles(ref)
    const raw = topic.trim().replace(/^\/+/, "")
    const rel = raw.endsWith(".md") ? raw : `${raw}.md`
    validatePath(rel)
    if (!(rel in files)) {
      const known = Object.keys(files).sort().join(", ")
      throw new Error(`No topic ${rel} in ${ref}.${known ? ` Topics: ${known}.` : ""}`)
    }
    const all = readEntries({ [rel]: files[rel]! })
    let start = 0
    if (options.after) {
      const at = all.findIndex((e) => e.entry.id === options.after)
      if (at < 0) throw new Error(`No entry ${options.after} in ${rel}; list the topic again without after.`)
      start = at + 1
    }
    const limit = Math.max(1, Math.min(options.limit ?? 200, 200))
    const page = all.slice(start, start + limit)
    const more = start + limit < all.length
    return { ref, path: rel, entries: page.map(listed), total: all.length, ...(more ? { next: page[page.length - 1]!.entry.id } : {}) }
  }

  /**
   * The system-prompt section for a session: each drive's MEMORY.md with its
   * checkout path, how to read topic files and how to save. Waits a bounded
   * time for session-start sync; never blocks a session on the network.
   */
  export async function sessionContext(options: { waitMs?: number; writeTool?: string; saveInstructions?: string; omitPersonalEntrypoint?: boolean } = {}): Promise<string> {
    if (!enabled()) return ""
    // The sync is a network pull; a turn waits for it only briefly (the
    // person sees "Thinking" meanwhile) and it carries on in the background.
    await Promise.race([prepare(), new Promise((r) => setTimeout(r, options.waitMs ?? 1_000))])
    // System prompts are rebuilt every turn: serve the last snapshot at once
    // and refresh it in the background when it's stale (git status + mounts
    // cost seconds under load); only the first build is waited for.
    const key = driveAccountSync().key
    let blocks: DriveContextBlock[]
    if (contextCache && contextCache.key === key) {
      blocks = contextCache.blocks
      if (Date.now() - contextCache.at >= CONTEXT_TTL_MS) refreshContext(key)
    } else {
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
  let refreshing: Promise<void> | undefined

  function refreshContext(key: string): void {
    if (refreshing) return
    // A write clears the cache meanwhile: then this older read is dropped.
    const base = contextCache
    refreshing = contextBlocks()
      .then((blocks) => {
        if (contextCache === base && driveAccountSync().key === key) contextCache = { key, at: Date.now(), blocks }
      })
      .catch(() => {})
      .finally(() => (refreshing = undefined))
  }

  export function toolSaveInstructions(writeTool: string): string {
    return `Save, update or delete memories with the \`${writeTool}\` tool. It adds the source, date and id, regenerates the index and syncs; do not edit MEMORY.md's index by hand. To change or delete an entry, pass the version memory_list or memory_recall gave you as if_version. Never store credentials, transcripts or raw logs.`
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
      "Before you ask the user something they may already have told you (a preference, a person, a project detail), check this index and the matching topic file first.",
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
      if (status?.needsLook) {
        lines.push(
          `${needsLookLine(status.needsLook)}: two devices changed the same memory, so both versions were kept (the copy is marked conflict_of). List them with memory_list and keep the right one.`,
        )
      }
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

/** The text part of a rendered entry line (`- <text> [meta]`). */
function parseText(line: string): string {
  const body = line.slice(2)
  const cut = body.lastIndexOf(" [")
  return cut < 0 ? body : body.slice(0, cut)
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
