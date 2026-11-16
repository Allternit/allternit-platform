/**
 * `gizzi files …`: the Files drive (contracts/files-drive/SPEC.md) from the
 * terminal. Verbs lead (design 6A, Eoj 2026-11-15); every verb takes `--json`.
 *
 *   gizzi files ls "Cloud computer":Inbox
 *   gizzi files put ./report.docx "Cloud computer":Inbox/
 *   gizzi files share Inbox/lease.pdf --role view_download --expires 7d
 *   gizzi files mount ~/Allternit --computer "Cloud computer"
 *
 * A path is `[computer:]Folder/sub/file`. Without a computer part it means
 * `--computer`, else this machine (when its runtime answers), else your only
 * paired computer.
 *
 * Browsing never wakes a computer: a sleeping one answers from its saved list.
 * Reading or writing content on a sleeping cloud computer asks first
 * ("Wake it? [y/N/always]"); `--wake` says yes, and "always" sets the
 * computer's wake setting to auto. `--json` never prompts: it prints the
 * `wake_consent_required` error for scripts to handle.
 *
 * Off unless the files_drive flag is on (ALLTERNIT_FILES_DRIVE=1): every verb
 * then prints "Files drive is not on for this account yet" and exits 2.
 */
import type { Argv } from "yargs"
import fs from "node:fs"
import fsp from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import readline from "node:readline"
import { spawn } from "node:child_process"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { cloudBase } from "@/cli/work/api"
import { FILES_DRIVE_OFF, filesDriveEnabled } from "@/cli/files/flag"
import {
  Drive,
  FilesError,
  READ_PIECE,
  SMALL_WRITE_MAX,
  formatBytes,
  formatItems,
  mountBinary,
  formatTime,
  freshnessLine,
  looksRemote,
  openDrive,
  parseExpiry,
  parseRemote,
  resolveParent,
  resolvePath,
  type Item,
  type OnConflict,
  type Resolved,
  type ShareRole,
  type WakeReason,
  type WakeSetting,
} from "@/cli/files/drive"

type Common = { json?: boolean; computer?: string; wake?: boolean; yes?: boolean }

const common = (y: Argv) =>
  y
    .option("json", { type: "boolean", describe: "print JSON (never prompts)" })
    .option("computer", { type: "string", alias: "c", describe: "computer name or id (default: this machine)" })

const withWake = (y: Argv) => common(y).option("wake", { type: "boolean", describe: "wake a sleeping computer without asking" })

function out(args: Common, json: unknown, text: string) {
  process.stdout.write((args.json ? JSON.stringify(json, null, 2) : text) + "\n")
}

/** Run one verb: plain error lines (or the ErrorBody as JSON) and exit 1 on failure. */
async function run(args: Common, fn: () => Promise<void>): Promise<void> {
  // Dark behind the files_drive flag (PLAN D10): one line, exit 2.
  if (!filesDriveEnabled()) {
    if (args.json) process.stdout.write(JSON.stringify({ error: { code: "flag_off", message: FILES_DRIVE_OFF, flag: "files_drive" } }) + "\n")
    else process.stderr.write(FILES_DRIVE_OFF + "\n")
    process.exitCode = 2
    return
  }
  try {
    await fn()
  } catch (e) {
    const err = e instanceof FilesError ? e : new FilesError("internal", 0, (e as Error)?.message ?? String(e))
    if (args.json) process.stdout.write(JSON.stringify(err.toJSON(), null, 2) + "\n")
    else {
      UI.error(err.code === "internal" || err.code === "offline" ? err.message : `${err.code}${err.status ? ` (${err.status})` : ""}: ${err.message}`)
      const hint = hintFor(err)
      if (hint) process.stderr.write(hint + "\n")
    }
    process.exitCode = 1
  }
}

function hintFor(err: FilesError): string | undefined {
  switch (err.code) {
    case "wake_consent_required":
      return "The computer is asleep. Run again with --wake, or: gizzi files wake-setting <computer> auto"
    case "wake_denied":
      return "Its wake setting is never. Change it with: gizzi files wake-setting <computer> ask"
    case "feature_disabled":
      return "The Files drive is not turned on for this account yet."
    case "protected_root_locked":
      return "This folder needs a device unlock (Face ID or biometrics). Open it in the Allternit app."
    case "rev_mismatch":
      return "It changed somewhere else since it was read. List it again and retry."
    default:
      return undefined
  }
}

function ask(question: string): Promise<string> {
  const rl = readline.createInterface({ input: process.stdin, output: process.stderr })
  return new Promise((resolve) => rl.question(question, (a) => (rl.close(), resolve(a.trim().toLowerCase()))))
}

/**
 * Call `fn`; when the computer is asleep and content needs it awake, get the
 * user's yes (or `--wake`), take a wake ticket and call again. Never wakes
 * without one of: `--wake`, a yes at the prompt, or the computer's setting `auto`.
 */
async function withConsent<T>(drive: Drive, args: Common, reason: WakeReason, fn: () => Promise<T>, itemId?: string): Promise<T> {
  try {
    return await fn()
  } catch (e) {
    if (!(e instanceof FilesError) || e.code !== "wake_consent_required") throw e
    const setting: WakeSetting | undefined = e.body?.wake?.setting
    let ok = args.wake === true || setting === "auto"
    if (!ok && !args.json && process.stdin.isTTY && process.stderr.isTTY) {
      const a = await ask(`${drive.name} is asleep. Wake it? About 40 seconds, billed at your plan's awake rate. [y/N/always] `)
      if (a === "always") {
        await drive.setWakeSetting("auto")
        ok = true
      } else ok = a === "y" || a === "yes"
    }
    if (!ok) throw e
    if (!args.json) process.stderr.write("waking …\n")
    const w = await drive.wakeConsent(reason, itemId)
    if (!args.json) process.stderr.write(`${w.computer_state}${w.estimated_wake_s ? ` (about ${w.estimated_wake_s} s)` : ""}\n`)
    return await fn()
  }
}

/** Open the computer a path names, and the path's segments. */
async function driveFor(arg: string | undefined, args: Common): Promise<{ drive: Drive; segments: string[]; trailingSlash: boolean }> {
  const p = parseRemote(arg ?? "")
  const drive = await openDrive(p.computer ?? args.computer)
  return { drive, segments: p.segments, trailingSlash: p.trailingSlash }
}

// ── ls ────────────────────────────────────────────────────────────────────

const LsCommand = cmd({
  command: "ls [path]",
  aliases: ["list"],
  describe: "list a computer's folders, or one folder (never wakes it)",
  builder: (y: Argv) => common(y).positional("path", { type: "string", describe: "[computer:]Folder/sub" }),
  handler: (args: any) =>
    run(args, async () => {
      const { drive, segments } = await driveFor(args.path, args)
      if (!segments.length) {
        const { roots, freshness } = await drive.rootsWithFreshness()
        const top = roots.filter((r) => !r.parent_root_id)
        const fresh = freshnessLine(freshness)
        const lines = top.map((r) => `${(r.name + "/").padEnd(24)} ${r.usage_bytes !== undefined ? formatBytes(r.usage_bytes) : ""}${r.policy.write ? "" : "  read-only"}${r.policy.protected ? "  protected" : ""}`)
        out(args, { computer_id: drive.computerId, roots, freshness }, [`${drive.name}${fresh ? `  ${fresh}` : ""}`, ...lines].join("\n"))
        return
      }
      const at = await resolvePath(drive, segments)
      if (!at.isFolder) {
        out(args, { item: at.item }, formatItems([at.item!], drive.device.id))
        return
      }
      const { items, freshness } = await drive.list(at.folder.rootId, at.folder.itemId)
      const fresh = freshnessLine(freshness)
      out(args, { computer_id: drive.computerId, root_id: at.folder.rootId, parent_id: at.folder.itemId, items, freshness }, (fresh ? fresh + "\n" : "") + formatItems(items, drive.device.id))
    }),
})

// ── cat / get ─────────────────────────────────────────────────────────────

async function fileAt(drive: Drive, segments: string[]): Promise<Item> {
  const at = await resolvePath(drive, segments)
  if (at.isFolder || !at.item) throw new FilesError("invalid_request", 400, `${at.name} is a folder. Use \`gizzi files ls\`.`)
  return at.item
}

/** Read a whole file in 4 MiB ranged pieces (one relay request each), calling `each` in order. */
async function readAll(drive: Drive, args: Common, item: Item, from: number, each: (bytes: Uint8Array, done: number) => Promise<void> | void) {
  let offset = from
  if (item.size === 0) return
  while (offset < item.size) {
    const piece = await withConsent(drive, args, "open_file", () => drive.readRange(item, offset, READ_PIECE), item.id)
    if (!piece.length) throw new FilesError("internal", 0, `${item.name}: the computer sent nothing at byte ${offset}.`)
    await each(piece, offset + piece.length)
    offset += piece.length
  }
}

const CatCommand = cmd({
  command: "cat <path>",
  describe: "print a file to stdout",
  builder: (y: Argv) => withWake(y).positional("path", { type: "string", demandOption: true }),
  handler: (args: any) =>
    run({ ...args, json: false }, async () => {
      const { drive, segments } = await driveFor(args.path, args)
      const item = await fileAt(drive, segments)
      await readAll(drive, { ...args, json: false }, item, 0, (b) => new Promise<void>((r) => (process.stdout.write(b) ? r() : process.stdout.once("drain", r))))
    }),
})

async function download(drive: Drive, args: Common & { resume?: boolean }, item: Item, dest: string): Promise<string> {
  let target = dest
  const st = await fsp.stat(target).catch(() => undefined)
  if (st?.isDirectory() || dest.endsWith("/")) target = path.join(dest, item.name)
  const part = `${target}.part`
  const have = args.resume ? ((await fsp.stat(part).catch(() => undefined))?.size ?? 0) : 0
  const fh = await fsp.open(part, have ? "a" : "w")
  const started = Date.now()
  try {
    await readAll(drive, args, item, Math.min(have, item.size), async (bytes, done) => {
      await fh.write(bytes)
      if (!args.json && process.stderr.isTTY) {
        const pct = item.size ? Math.floor((done / item.size) * 100) : 100
        const rate = (done - have) / Math.max(0.001, (Date.now() - started) / 1000)
        process.stderr.write(`\r${item.name}  ${pct}%  ${formatBytes(rate)}/s   `)
      }
    })
  } finally {
    await fh.close()
  }
  if (!args.json && process.stderr.isTTY) process.stderr.write("\n")
  await fsp.rename(part, target)
  return target
}

const GetCommand = cmd({
  command: "get <path> [dest]",
  describe: "download a file (resumes with --resume)",
  builder: (y: Argv) =>
    withWake(y)
      .positional("path", { type: "string", demandOption: true })
      .positional("dest", { type: "string", describe: "local file or folder (default: here)" })
      .option("resume", { type: "boolean", describe: "continue a download that stopped (<file>.part)" }),
  handler: (args: any) =>
    run(args, async () => {
      const { drive, segments } = await driveFor(args.path, args)
      const item = await fileAt(drive, segments)
      const saved = await download(drive, args, item, args.dest ?? ".")
      out(args, { item, saved_to: path.resolve(saved) }, `saved ${saved}  ${formatBytes(item.size)}`)
    }),
})

// ── put ───────────────────────────────────────────────────────────────────

const NO_BINARY =
  "allternit-files-mount is missing. It ships with gizzi (npm, brew, release tarball) and with Allternit Desktop on macOS and Linux; reinstall one, or set ALLTERNIT_FILES_MOUNT_BIN to its path."

/** The `ALLTERNIT_FILES_*` connection for the mount binary (tokens by environment, never argv). */
async function mountEnv(drive: Drive, allowWake: boolean): Promise<Record<string, string>> {
  const t = drive.transport
  const env: Record<string, string> = {
    ALLTERNIT_FILES_COMPUTER_ID: drive.computerId,
    ALLTERNIT_FILES_TRANSPORT: t.kind,
    ALLTERNIT_FILES_BASE_URL: t.kind === "relay" ? t.proxyUrl : t.baseUrl,
    ALLTERNIT_FILES_CLOUD_URL: cloudBase(),
    ALLTERNIT_FILES_DEVICE_ID: drive.device.id,
    ALLTERNIT_FILES_DEVICE_NAME: drive.device.name,
    ALLTERNIT_FILES_ALLOW_WAKE: allowWake ? "1" : "0",
  }
  const auth = t.token
  if (auth) env.ALLTERNIT_FILES_AUTH = auth
  if (t.kind !== "relay" && t.userId) env.ALLTERNIT_FILES_USER_ID = t.userId
  const cloud = process.env.ALLTERNIT_API_TOKEN?.trim() || drive.device.token
  if (cloud) env.ALLTERNIT_FILES_CLOUD_AUTH = cloud
  const ticket = drive.wakeTicket()
  if (ticket) {
    env.ALLTERNIT_FILES_WAKE_TICKET = ticket.value
    env.ALLTERNIT_FILES_WAKE_TICKET_EXPIRES = ticket.expiresAt
  }
  return env
}

/** A file over 4 MiB: the mount binary's upload session with the pinned chunker (SPEC section 5). */
async function bigUpload(drive: Drive, args: Common, src: string, rootId: string, parentId: string, name: string, onConflict: OnConflict, replace?: Item): Promise<Item> {
  const bin = mountBinary()
  if (!bin) throw new FilesError("unsupported", 501, `${path.basename(src)} is over 4 MB. ${NO_BINARY}`)
  const argv = ["put", src, "--root", rootId, "--parent", parentId, "--name", name, "--on-conflict", onConflict]
  if (replace) argv.push("--replace", replace.id, "--base-rev", replace.rev)
  // Ask before waking here, once, so the binary carries the ticket instead of asking again.
  const env = { ...process.env, ...(await mountEnv(drive, args.wake === true)) }
  const result = await new Promise<{ code: number; stdout: string }>((resolve, reject) => {
    const child = spawn(bin, argv, { env, stdio: ["ignore", "pipe", "inherit"] })
    let stdout = ""
    child.stdout.on("data", (d) => (stdout += d))
    child.on("error", reject)
    child.on("close", (code) => resolve({ code: code ?? 1, stdout }))
  })
  const last = result.stdout.trim().split("\n").pop() ?? ""
  let body: any = {}
  try {
    body = JSON.parse(last)
  } catch {}
  if (result.code !== 0 || !body.item) throw new FilesError(body.code ?? "internal", body.status ?? 0, body.message ?? `upload failed (exit ${result.code})`)
  return body.item as Item
}

async function upload(drive: Drive, args: Common & { onConflict?: OnConflict }, src: string, remote: { segments: string[]; trailingSlash: boolean }): Promise<{ item: Item; conflict?: string }> {
  const st = await fsp.stat(src).catch(() => undefined)
  if (!st?.isFile()) throw new FilesError("invalid_request", 400, `${src} is not a file.`)
  // Destination: an existing folder (put into it), an existing file (replace it), or a new name.
  let rootId: string, parentId: string, name: string, replace: Item | undefined
  const existing = await resolvePath(drive, remote.segments).catch((e: FilesError) => (e.code === "not_found" ? undefined : Promise.reject(e)))
  if (existing?.isFolder) {
    ;[rootId, parentId, name] = [existing.folder.rootId, existing.folder.itemId, path.basename(src)]
    const { items } = await drive.list(rootId, parentId)
    replace = items.find((i) => i.name === name && i.kind === "file")
  } else if (existing?.item) {
    ;[rootId, parentId, name, replace] = [existing.item.root_id, existing.item.parent_id ?? existing.folder.itemId, existing.item.name, existing.item]
  } else {
    if (remote.trailingSlash) throw new FilesError("not_found", 404, `No folder ${remote.segments.join("/")}.`)
    const { parent, name: n } = await resolveParent(drive, remote.segments)
    ;[rootId, parentId, name] = [parent.folder.rootId, parent.folder.itemId, n]
  }
  const onConflict: OnConflict = args.onConflict ?? "keep_both"
  if (st.size > SMALL_WRITE_MAX) {
    // Get consent (if needed) with a cheap call first, so the binary carries the ticket.
    await withConsent(drive, args, "upload", () => drive.json("POST", "/chunks/have-need", { hashes: ["0".repeat(64)] }))
    return { item: await bigUpload(drive, args, src, rootId, parentId, name, onConflict, replace) }
  }
  const bytes = new Uint8Array(await fsp.readFile(src))
  const m = await withConsent(drive, args, "upload", () => drive.smallWrite(rootId, parentId, name, bytes, onConflict, replace))
  return { item: m.item, conflict: m.conflict?.conflict_name }
}

const conflictOption = (y: Argv) =>
  y.option("on-conflict", { type: "string", choices: ["keep_both", "fail", "replace"], describe: "when the name exists or it changed elsewhere (default keep_both)" })

const PutCommand = cmd({
  command: "put <src> <dest>",
  describe: "upload a local file to a folder or name",
  builder: (y: Argv) => conflictOption(withWake(y).positional("src", { type: "string", demandOption: true }).positional("dest", { type: "string", demandOption: true })),
  handler: (args: any) =>
    run(args, async () => {
      const { drive, segments, trailingSlash } = await driveFor(args.dest, args)
      const r = await upload(drive, args, args.src, { segments, trailingSlash })
      out(args, r, `${r.item.name}  ${formatBytes(r.item.size)}  uploaded${r.conflict ? `\nchanged elsewhere: saved as ${r.conflict}` : ""}`)
    }),
})

const CpCommand = cmd({
  command: "cp <src> <dest>",
  describe: "copy: local → computer (put), computer → local (get), or within a computer. Mark remote paths computer: or :",
  builder: (y: Argv) =>
    conflictOption(withWake(y).positional("src", { type: "string", demandOption: true }).positional("dest", { type: "string", demandOption: true })).option("resume", {
      type: "boolean",
      describe: "continue a download that stopped",
    }),
  handler: (args: any) =>
    run(args, async () => {
      const [srcRemote, destRemote] = [looksRemote(args.src), looksRemote(args.dest)]
      if (!srcRemote && !destRemote) throw new FilesError("invalid_request", 400, 'Mark the computer side, e.g. gizzi files cp ./a.pdf "My computer":Inbox/  (or :Inbox/ for the default computer).')
      if (!srcRemote) {
        const { drive, segments, trailingSlash } = await driveFor(args.dest, args)
        const r = await upload(drive, args, args.src, { segments, trailingSlash })
        out(args, r, `${r.item.name}  ${formatBytes(r.item.size)}  uploaded${r.conflict ? `\nchanged elsewhere: saved as ${r.conflict}` : ""}`)
        return
      }
      const src = await driveFor(args.src, args)
      if (!destRemote) {
        const item = await fileAt(src.drive, src.segments)
        const saved = await download(src.drive, args, item, args.dest)
        out(args, { item, saved_to: path.resolve(saved) }, `saved ${saved}  ${formatBytes(item.size)}`)
        return
      }
      const destP = parseRemote(args.dest)
      if ((destP.computer ?? args.computer) !== (parseRemote(args.src).computer ?? args.computer))
        throw new FilesError("unsupported", 501, "Copying between two computers: get it, then put it (gizzi files get …, gizzi files put …).")
      const at = await resolvePath(src.drive, src.segments)
      if (!at.item) throw new FilesError("invalid_request", 400, "A computer's main folders can't be copied.")
      const dest = await resolvePath(src.drive, destP.segments).catch((e: FilesError) => (e.code === "not_found" ? undefined : Promise.reject(e)))
      const target = dest?.isFolder ? { parent: dest, name: undefined as string | undefined } : await resolveParent(src.drive, destP.segments)
      const r = await withConsent(src.drive, args, "sync", () => src.drive.copy(at.item!, target.parent.folder.itemId, target.name, args.onConflict ?? "keep_both"))
      out(args, r, r.item ? `copied to ${r.item.name}` : `copying in the background (job ${r.job?.job_id})`)
    }),
})

// ── mv / rm / mkdir ───────────────────────────────────────────────────────

const MvCommand = cmd({
  command: "mv <src> <dest>",
  describe: "rename or move within one computer",
  builder: (y: Argv) => common(y).positional("src", { type: "string", demandOption: true }).positional("dest", { type: "string", demandOption: true }).option("replace", { type: "boolean", describe: "replace a file already at dest" }),
  handler: (args: any) =>
    run(args, async () => {
      const { drive, segments } = await driveFor(args.src, args)
      const at = await resolvePath(drive, segments)
      if (!at.item || at.item.root_link) throw new FilesError("forbidden", 403, "A computer's main folders can't be renamed or moved.")
      const destSegs = parseRemote(args.dest).segments
      const dest = await resolvePath(drive, destSegs).catch((e: FilesError) => (e.code === "not_found" ? undefined : Promise.reject(e)))
      let to: { parentId?: string; name?: string }
      if (dest?.isFolder) to = { parentId: dest.folder.itemId }
      else {
        const { parent, name } = await resolveParent(drive, destSegs)
        to = { parentId: parent.folder.itemId === at.item.parent_id ? undefined : parent.folder.itemId, name: name === at.item.name ? undefined : name }
      }
      if (!to.parentId && !to.name) {
        out(args, { item: at.item }, "nothing to move")
        return
      }
      const m = await drive.move(at.item, to, args.replace ? "replace" : "fail")
      out(args, m, `moved to ${m.item.name}`)
    }),
})

const RmCommand = cmd({
  command: "rm <path>",
  describe: "move to trash (--permanent deletes for good)",
  builder: (y: Argv) => common(y).positional("path", { type: "string", demandOption: true }).option("permanent", { type: "boolean" }).option("yes", { type: "boolean", alias: "y", describe: "don't ask before --permanent" }),
  handler: (args: any) =>
    run(args, async () => {
      const { drive, segments } = await driveFor(args.path, args)
      const at = await resolvePath(drive, segments)
      if (!at.item || at.item.root_link) throw new FilesError("forbidden", 403, "A computer's main folders can't be removed.")
      if (args.permanent && !args.yes) {
        if (args.json || !process.stdin.isTTY) throw new FilesError("invalid_request", 400, "--permanent needs --yes when nothing can ask.")
        if ((await ask(`Delete ${at.item.name} for good? It skips the trash. [y/N] `)) !== "y") return
      }
      const r = await drive.delete(at.item, args.permanent === true)
      out(args, r, r.trashed ? `${at.item.name} moved to trash (until ${formatTime(r.trashed.purge_at)})` : `${at.item.name} deleted`)
    }),
})

const MkdirCommand = cmd({
  command: "mkdir <path>",
  describe: "make a folder (-p makes missing parents)",
  builder: (y: Argv) => common(y).positional("path", { type: "string", demandOption: true }).option("parents", { type: "boolean", alias: "p" }),
  handler: (args: any) =>
    run(args, async () => {
      const { drive, segments } = await driveFor(args.path, args)
      if (!args.parents) {
        const { parent, name } = await resolveParent(drive, segments)
        const m = await drive.mkdir(parent.folder.itemId, name)
        out(args, m, `${name}/ created`)
        return
      }
      let made: Item | undefined
      for (let i = 2; i <= segments.length; i++) {
        const exists = await resolvePath(drive, segments.slice(0, i)).catch((e: FilesError) => (e.code === "not_found" ? undefined : Promise.reject(e)))
        if (exists) continue
        const { parent, name } = await resolveParent(drive, segments.slice(0, i))
        made = (await drive.mkdir(parent.folder.itemId, name)).item
      }
      out(args, { item: made ?? null }, made ? `${segments.join("/")}/ created` : "already there")
    }),
})

// ── share ─────────────────────────────────────────────────────────────────

const ShareCommand = cmd({
  command: "share [path]",
  describe: "make a share link for a folder or file (--list shows links, --revoke ends one)",
  builder: (y: Argv) =>
    common(y)
      .positional("path", { type: "string" })
      .option("role", { type: "string", choices: ["view_download", "upload_only", "edit", "comment"], default: "view_download" })
      .option("expires", { type: "string", describe: "7d, 12h, 30m or a date" })
      .option("passcode", { type: "boolean", describe: "ask for a passcode guests must type" })
      .option("guest-wake", { type: "boolean", describe: "let guests wake the computer (billed to you)" })
      .option("invite", { type: "array", string: true, describe: "email addresses to invite" })
      .option("list", { type: "boolean", describe: "list this computer's share links" })
      .option("revoke", { type: "string", describe: "share id to revoke" }),
  handler: (args: any) =>
    run(args, async () => {
      const { drive, segments } = await driveFor(args.path, args)
      if (args.revoke) {
        const res = await fetch(`${cloudBase()}/api/v1/computer-files/shares/${encodeURIComponent(args.revoke)}`, {
          method: "DELETE",
          headers: { Authorization: `Bearer ${process.env.ALLTERNIT_API_TOKEN?.trim() || drive.device.token || ""}` },
          signal: AbortSignal.timeout(20_000),
        })
        if (!res.ok && res.status !== 204) throw new FilesError(res.status === 404 ? "not_found" : "internal", res.status, `revoke: ${res.status}`)
        out(args, { revoked: args.revoke }, `revoked ${args.revoke}; it stops working on the guest's next request`)
        return
      }
      if (args.list || !segments.length) {
        const { shares } = await drive.listShares()
        const live = shares.filter((s) => !s.revoked_at)
        out(
          args,
          { shares },
          live.length
            ? live.map((s) => `${s.share_id}  ${s.role.padEnd(13)} ${s.expires_at ? `until ${formatTime(s.expires_at)}` : "no expiry"}  ${s.guest_count} guests  ${s.link_url}`).join("\n")
            : "no share links",
        )
        return
      }
      const at: Resolved = await resolvePath(drive, segments)
      let passcode: string | undefined
      if (args.passcode) {
        if (args.json || !process.stdin.isTTY) throw new FilesError("invalid_request", 400, "--passcode asks for it at the prompt; run it in a terminal.")
        passcode = await ask("Passcode: ")
      }
      const { share } = await drive.createShare({
        root_id: at.folder.rootId,
        item_id: at.item?.id ?? at.folder.itemId,
        role: args.role as ShareRole,
        expires_at: parseExpiry(args.expires),
        passcode,
        guest_wake: args.guestWake === true,
        invite_emails: args.invite?.length ? args.invite : undefined,
      })
      out(
        args,
        { share },
        `link ${share.link_url}\nrole ${share.role} · ${share.expires_at ? `expires ${formatTime(share.expires_at)}` : "no expiry"} · ${share.guest_wake ? "guests may wake it" : "no wake for guests"}${share.passcode_required ? " · passcode" : ""}`,
      )
    }),
})

// ── pin / unpin ───────────────────────────────────────────────────────────

function pinCommand(pinned: boolean) {
  return cmd({
    command: `${pinned ? "pin" : "unpin"} <path>`,
    describe: pinned ? "keep a file or folder on this device offline" : "stop keeping it offline here",
    builder: (y: Argv) => common(y).positional("path", { type: "string", demandOption: true }),
    handler: (args: any) =>
      run(args, async () => {
        const { drive, segments } = await driveFor(args.path, args)
        const at = await resolvePath(drive, segments)
        if (!at.item) throw new FilesError("invalid_request", 400, "Pin a folder or file inside one of the main folders.")
        const r = await drive.pin(at.item, pinned)
        out(args, r, `${at.item.name} ${pinned ? "kept offline on" : "no longer offline on"} ${drive.device.name}`)
      }),
  })
}

// ── versions / trash / restore ────────────────────────────────────────────

const VersionsCommand = cmd({
  command: "versions <a> [b] [c]",
  describe: "list a file's versions; `versions restore <path> <n>` brings one back",
  builder: (y: Argv) => common(y).positional("a", { type: "string", describe: "path, or `restore`" }),
  handler: (args: any) =>
    run(args, async () => {
      const restoring = args.a === "restore"
      const p = restoring ? args.b : args.a
      if (!p) throw new FilesError("invalid_request", 400, "Usage: gizzi files versions <path>  or  gizzi files versions restore <path> <n>")
      const { drive, segments } = await driveFor(p, args)
      const item = await fileAt(drive, segments)
      const versions = await drive.versions(item)
      if (!restoring) {
        const lines = versions.map(
          (v, i) => `${String(i + 1).padStart(2)}  ${formatTime(v.created_at).padEnd(18)} ${(v.created_by.display_name ?? v.created_by.kind).padEnd(20)} ${formatBytes(v.size)}${v.rev === item.rev ? "  current" : ""}`,
        )
        out(args, { item_id: item.id, versions }, lines.length ? lines.join("\n") : "no earlier versions")
        return
      }
      const which = String(args.c ?? "")
      const v = versions.find((x) => x.version_id === which) ?? versions[Number(which) - 1]
      if (!v) throw new FilesError("not_found", 404, `No version ${which}. List them with: gizzi files versions ${p}`)
      const m = await withConsent(drive, args, "sync", () => drive.restoreVersion(item, v.version_id), item.id)
      out(args, m, `${item.name} restored to the version from ${formatTime(v.created_at)}`)
    }),
})

async function allTrash(drive: Drive, rootName?: string) {
  const roots = await drive.roots()
  const pickRoots = rootName ? roots.filter((r) => r.name.toLowerCase() === rootName.toLowerCase()) : roots
  if (rootName && !pickRoots.length) throw new FilesError("root_not_found", 404, `No folder "${rootName}".`)
  const lists = await Promise.all(pickRoots.map((r) => drive.trash(r.id).catch(() => [])))
  return lists.flat()
}

async function restoreFromTrash(drive: Drive, args: any, key: string) {
  const entries = await allTrash(drive)
  const hits = entries.filter((e) => e.trash_id === key || e.item.name === key)
  if (!hits.length) throw new FilesError("not_found", 404, `Nothing named "${key}" in the trash. See: gizzi files trash ls`)
  if (hits.length > 1) throw new FilesError("invalid_request", 400, `${hits.length} items named "${key}" are in the trash. Restore one by id: ${hits.map((h) => h.trash_id).join(", ")}`)
  const m = await drive.restore(hits[0].trash_id, args.onConflict ?? "keep_both")
  out(args, m, `${m.item.name} restored`)
}

const TrashCommand = cmd({
  command: "trash [action] [target]",
  describe: "`trash ls [folder]` lists the trash; `trash restore <name or id>` brings one back",
  builder: (y: Argv) => conflictOption(common(y).positional("action", { type: "string", choices: ["ls", "restore"], default: "ls" })),
  handler: (args: any) =>
    run(args, async () => {
      const drive = await openDrive(args.computer)
      if (args.action === "restore") {
        if (!args.target) throw new FilesError("invalid_request", 400, "Usage: gizzi files trash restore <name or trash id>")
        await restoreFromTrash(drive, args, args.target)
        return
      }
      const entries = await allTrash(drive, args.target)
      const now = Date.now()
      out(
        args,
        { entries },
        entries.length
          ? entries.map((e) => `${e.item.name.padEnd(36)} ${Math.max(0, Math.ceil((Date.parse(e.purge_at) - now) / 86_400_000))} days left  ${e.trash_id}`).join("\n")
          : "trash is empty",
      )
    }),
})

const RestoreCommand = cmd({
  command: "restore <target>",
  describe: "bring a file back from the trash (name or trash id)",
  builder: (y: Argv) => conflictOption(common(y).positional("target", { type: "string", demandOption: true })),
  handler: (args: any) => run(args, async () => restoreFromTrash(await openDrive(args.computer), args, args.target)),
})

// ── search ────────────────────────────────────────────────────────────────

const SearchCommand = cmd({
  command: "search <query>",
  describe: "find files by name (content and meaning search need the computer awake)",
  builder: (y: Argv) =>
    withWake(y)
      .positional("query", { type: "string", demandOption: true })
      .option("mode", { type: "string", choices: ["name", "full_text", "semantic"], default: "name" })
      .option("in", { type: "string", describe: "only this folder (root name)" }),
  handler: (args: any) =>
    run(args, async () => {
      const drive = await openDrive(args.computer)
      const roots = args.in ? (await drive.roots()).filter((r) => r.name.toLowerCase() === String(args.in).toLowerCase()).map((r) => r.id) : undefined
      const r = await withConsent(drive, args, "search_content", () => drive.search(args.query, args.mode, roots))
      const fresh = freshnessLine(r.freshness)
      out(args, r, (fresh ? fresh + "\n" : "") + (r.hits.length ? formatItems(r.hits.map((h) => h.item), drive.device.id) : "no matches") + (r.index_state !== "ready" ? `\nindex: ${r.index_state}` : ""))
    }),
})

// ── wake ──────────────────────────────────────────────────────────────────

const WakeCommand = cmd({
  command: "wake [target]",
  describe: "wake a sleeping cloud computer (asks first; --yes skips the question)",
  builder: (y: Argv) => common(y).positional("target", { type: "string", describe: "computer name or id" }).option("yes", { type: "boolean", alias: "y" }),
  handler: (args: any) =>
    run(args, async () => {
      const drive = await openDrive(args.target ?? args.computer)
      if (!drive.wakeable) {
        out(args, { computer_id: drive.computerId, computer_state: "awake", woke: false }, `${drive.name} is your own device; it can't be woken from here.`)
        return
      }
      if (!args.yes) {
        if (args.json || !process.stdin.isTTY) throw new FilesError("invalid_request", 400, "wake needs --yes when nothing can ask.")
        const a = await ask(`Wake ${drive.name}? About 40 seconds, billed at your plan's awake rate. [y/N] `)
        if (a !== "y" && a !== "yes") return
      }
      const w = await drive.wakeConsent("sync")
      out(args, { computer_id: drive.computerId, ...w, woke: true }, `${drive.name}: ${w.computer_state}${w.estimated_wake_s ? ` (about ${w.estimated_wake_s} s)` : ""}`)
    }),
})

const WakeSettingCommand = cmd({
  command: "wake-setting [target] [setting]",
  describe: "show or set when Files may wake a computer: ask (default), auto, never",
  builder: (y: Argv) =>
    common(y).positional("target", { type: "string", describe: "computer name or id" }).positional("setting", { type: "string", choices: ["ask", "auto", "never"] }),
  handler: (args: any) =>
    run(args, async () => {
      const drive = await openDrive(args.target ?? args.computer)
      const s = args.setting ? await drive.setWakeSetting(args.setting) : await drive.getWakeSetting()
      out(args, s, `${drive.name}: wake ${s.wake}`)
    }),
})

// ── mount / unmount ───────────────────────────────────────────────────────

type MountRecord = { mountpoint?: string; url?: string; kind: "fuse" | "webdav"; pid: number; computer_id: string; name: string; started_at: string }
const MOUNTS_FILE = path.join(os.homedir(), ".allternit", "state", "files-mounts.json")

async function readMounts(): Promise<MountRecord[]> {
  const list: MountRecord[] = JSON.parse(await fsp.readFile(MOUNTS_FILE, "utf8").catch(() => "[]"))
  // Drop mounts whose process is gone.
  return list.filter((m) => {
    try {
      process.kill(m.pid, 0)
      return true
    } catch {
      return false
    }
  })
}

async function writeMounts(list: MountRecord[]) {
  await fsp.mkdir(path.dirname(MOUNTS_FILE), { recursive: true })
  await fsp.writeFile(MOUNTS_FILE, JSON.stringify(list, null, 2), { mode: 0o600 })
}

const MountCommand = cmd({
  command: "mount [mountpoint]",
  describe: "show a computer's drive as a folder: FUSE on Linux, else a local WebDAV address (--webdav). No args lists mounts",
  builder: (y: Argv) =>
    withWake(y)
      .positional("mountpoint", { type: "string" })
      .option("webdav", { type: "boolean", describe: "serve WebDAV on 127.0.0.1 instead of a FUSE mount" })
      .option("read-only", { type: "boolean" }),
  handler: (args: any) =>
    run(args, async () => {
      if (!args.mountpoint && !args.webdav) {
        const mounts = await readMounts()
        out(args, { mounts }, mounts.length ? mounts.map((m) => `${m.kind.padEnd(6)} ${m.mountpoint ?? m.url}  ${m.name}  pid ${m.pid}`).join("\n") : "no mounts")
        return
      }
      const bin = mountBinary()
      if (!bin) throw new FilesError("unsupported", 501, NO_BINARY)
      const webdav = args.webdav === true || process.platform !== "linux"
      if (!webdav && !Bun.which("fusermount3") && !Bun.which("fusermount"))
        throw new FilesError("unsupported", 501, "FUSE is not installed here (no fusermount3). Install fuse3, or use --webdav.")
      const drive = await openDrive(args.computer)
      // Opening files on a sleeping cloud computer wakes it: ask once now, never per file.
      let allowWake = args.wake === true
      if (!allowWake && drive.wakeable && !args.json && process.stdin.isTTY) {
        const s = await drive.getWakeSetting().catch(() => undefined)
        if (s?.wake === "auto") allowWake = true
        else if (s?.wake !== "never")
          allowWake = ["y", "yes"].includes(await ask(`Wake ${drive.name} when you open a file in the mount? Browsing never wakes it. [y/N] `))
      }
      const env = { ...process.env, ...(await mountEnv(drive, allowWake)) }
      const mp = args.mountpoint ? path.resolve(args.mountpoint.replace(/^~(?=$|\/)/, os.homedir())) : undefined
      const argv = webdav ? ["dav", ...(args.readOnly ? ["--read-only"] : [])] : ["fuse", mp!, "--name", drive.name, ...(args.readOnly ? ["--read-only"] : [])]
      if (!webdav) await fsp.mkdir(mp!, { recursive: true })
      const logPath = path.join(os.homedir(), ".allternit", "logs", `files-mount-${drive.computerId}.log`)
      await fsp.mkdir(path.dirname(logPath), { recursive: true })
      const log = fs.openSync(logPath, "a")
      const child = spawn(bin, argv, { env, detached: true, stdio: ["ignore", webdav ? "pipe" : "ignore", log] })
      let url: string | undefined
      if (webdav) {
        url = await new Promise<string>((resolve, reject) => {
          let buf = ""
          const timer = setTimeout(() => reject(new FilesError("internal", 0, `WebDAV server did not start; see ${logPath}`)), 15_000)
          child.stdout!.on("data", (d) => {
            buf += d
            const line = buf.split("\n")[0]
            try {
              const j = JSON.parse(line)
              clearTimeout(timer)
              resolve(j.url)
            } catch {}
          })
          child.on("exit", (code) => (clearTimeout(timer), reject(new FilesError("internal", 0, `WebDAV server exited (${code}); see ${logPath}`))))
        })
        child.stdout!.destroy()
      } else {
        // Give fusermount a moment; a failed mount exits fast.
        const early = await Promise.race([new Promise<number>((r) => child.on("exit", (c) => r(c ?? 1))), Bun.sleep(1500).then(() => -1)])
        if (early !== -1) throw new FilesError("internal", 0, `mount failed (exit ${early}); see ${logPath}`)
      }
      child.unref()
      const rec: MountRecord = { kind: webdav ? "webdav" : "fuse", mountpoint: mp, url, pid: child.pid!, computer_id: drive.computerId, name: drive.name, started_at: new Date().toISOString() }
      await writeMounts([...(await readMounts()).filter((m) => m.pid !== rec.pid), rec])
      out(
        args,
        rec,
        webdav
          ? `serving ${url!.replace(/^http:/, "dav:")}\nOpen it in your file manager (GNOME/KDE: dav://…, or davfs2/rclone). The address holds a secret; don't share it.`
          : `mounted ${mp}  (FUSE · ${drive.name}${allowWake ? " · opens wake it" : " · browse works while asleep"})`,
      )
    }),
})

const UnmountCommand = cmd({
  command: "unmount [target]",
  aliases: ["umount"],
  describe: "stop a mount (mount point or WebDAV address; none = all)",
  builder: (y: Argv) => common(y).positional("target", { type: "string" }),
  handler: (args: any) =>
    run(args, async () => {
      const mounts = await readMounts()
      const target = args.target ? path.resolve(String(args.target).replace(/^~(?=$|\/)/, os.homedir())) : undefined
      const pickM = target ? mounts.filter((m) => m.mountpoint === target || m.url === args.target) : mounts
      if (!pickM.length) throw new FilesError("not_found", 404, target ? `Nothing is mounted at ${args.target}.` : "No mounts.")
      for (const m of pickM) {
        if (m.kind === "fuse" && m.mountpoint) {
          const tool = Bun.which("fusermount3") ?? Bun.which("fusermount")
          if (tool) await new Promise((r) => spawn(tool, ["-u", m.mountpoint!], { stdio: "ignore" }).on("close", r))
        }
        try {
          process.kill(m.pid, "SIGTERM")
        } catch {}
      }
      await writeMounts(mounts.filter((m) => !pickM.includes(m)))
      out(args, { unmounted: pickM }, pickM.map((m) => `unmounted ${m.mountpoint ?? m.url}`).join("\n"))
    }),
})

// ── the command ───────────────────────────────────────────────────────────

export const FilesCommand = cmd({
  command: "files",
  describe: "your computers' files: ls, cat, get, put, cp, mv, rm, mkdir, share, pin, versions, trash, search, wake, mount",
  builder: (yargs: Argv) =>
    yargs
      .command(LsCommand)
      .command(CatCommand)
      .command(GetCommand)
      .command(PutCommand)
      .command(CpCommand)
      .command(MvCommand)
      .command(RmCommand)
      .command(MkdirCommand)
      .command(ShareCommand)
      .command(pinCommand(true))
      .command(pinCommand(false))
      .command(VersionsCommand)
      .command(TrashCommand)
      .command(RestoreCommand)
      .command(SearchCommand)
      .command(WakeCommand)
      .command(WakeSettingCommand)
      .command(MountCommand)
      .command(UnmountCommand)
      .demandCommand(1, "Specify a files command (ls, cat, get, put, cp, mv, rm, mkdir, share, pin, unpin, versions, trash, restore, search, wake, wake-setting, mount, unmount)"),
  handler: () => {},
})
