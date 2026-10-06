import { afterAll, describe, expect, test } from "bun:test"
import { mkdtemp, readFile, rm, writeFile, mkdir, readdir } from "fs/promises"
import { existsSync } from "fs"
import os from "os"
import path from "path"
import { DriveCheckout, type DriveRemote } from "../../../src/runtime/memory/drive/checkout"
import { deriveEntryId, parseEntry, rustLines, type Entry } from "../../../src/runtime/memory/drive/format"
import { runGit } from "../../../src/runtime/memory/drive/git"

const roots: string[] = []
afterAll(async () => {
  for (const r of roots) await rm(r, { recursive: true, force: true })
})

async function scratch(): Promise<string> {
  const dir = await mkdtemp(path.join(os.tmpdir(), "gizzi-drive-"))
  roots.push(dir)
  return dir
}

const author = { name: "gizzi test", email: "gizzi@test.local" }

async function bareRemote(root: string): Promise<string> {
  const remote = path.join(root, "remote.git")
  await mkdir(remote)
  const r = await runGit(["init", "--bare", "--quiet"], { cwd: remote })
  expect(r.code).toBe(0)
  await runGit(["symbolic-ref", "HEAD", "refs/heads/main"], { cwd: remote })
  return remote
}

function remoteFor(url: string, extra: Partial<DriveRemote> = {}): DriveRemote {
  return { url, token: async () => "test-token-not-a-secret", ...extra }
}

function entry(id: string, text: string, source = "gizzi:session/ses_1"): Entry {
  return { id, text, source, added: "2026-10-06", metadata: {} }
}

async function remoteFiles(remote: string): Promise<Record<string, string>> {
  const ls = await runGit(["ls-tree", "-r", "--name-only", "main"], { cwd: remote })
  const out: Record<string, string> = {}
  for (const p of ls.stdout.split("\n").filter(Boolean)) {
    out[p] = (await runGit(["show", `main:${p}`], { cwd: remote })).stdout
  }
  return out
}

async function remoteLog(remote: string): Promise<string[]> {
  return (await runGit(["log", "--format=%s", "main"], { cwd: remote })).stdout.split("\n").filter(Boolean)
}

describe("memory drive checkout", () => {
  test("first write creates a standard repo, one commit, and pushes", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    const drive = new DriveCheckout({ dir: path.join(root, "a"), remote: remoteFor(remote), author })
    const result = await drive.write([{ UpsertEntry: { path: "preferences.md", entry: entry("pref-1", "Prefers short answers") } }], {
      message: "Remember a preference",
    })
    expect(result.pushed).toBe(true)
    expect(result.pending).toBe(false)
    const files = await remoteFiles(remote)
    expect(files["MEMORY.md"]).toBe("# Memory\n\n## Index\n- [[preferences]]")
    expect(files["preferences.md"]).toContain("- Prefers short answers [source: gizzi:session/ses_1; added: 2026-10-06; id: pref-1]")
    expect(await remoteLog(remote)).toEqual(["Remember a preference", "Initialize memory drive"])
    // Working tree holds the same files for normal read tools.
    expect(await readFile(path.join(root, "a", "preferences.md"), "utf8")).toContain("pref-1")
  })

  test("two sessions write different entries: both land", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    const a = new DriveCheckout({ dir: path.join(root, "a"), remote: remoteFor(remote), author })
    const b = new DriveCheckout({ dir: path.join(root, "b"), remote: remoteFor(remote), author })
    await a.ensure()
    await b.ensure()
    await a.write([{ UpsertEntry: { path: "notes.md", entry: entry("a-1", "Session A fact") } }], { message: "A" })
    // b's checkout is now behind; its write integrates and both entries survive.
    const r = await b.write([{ UpsertEntry: { path: "notes.md", entry: entry("b-1", "Session B fact") } }], { message: "B" })
    expect(r.pushed).toBe(true)
    const notes = (await remoteFiles(remote))["notes.md"]!
    expect(notes).toContain("id: a-1")
    expect(notes).toContain("id: b-1")
  })

  test("concurrent writers from separate checkouts: rejected push is re-read and nothing is lost", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    const a = new DriveCheckout({ dir: path.join(root, "a"), remote: remoteFor(remote), author })
    const b = new DriveCheckout({ dir: path.join(root, "b"), remote: remoteFor(remote), author })
    await a.ensure()
    await b.ensure()
    // Both start from the same head and race; one push is rejected and retried.
    const [ra, rb] = await Promise.all([
      a.write([{ UpsertEntry: { path: "notes.md", entry: entry("same-id", "Version from A") } }, { UpsertEntry: { path: "a.md", entry: entry("a-only", "Only A") } }], { message: "A" }),
      b.write([{ UpsertEntry: { path: "notes.md", entry: entry("same-id", "Version from B") } }, { UpsertEntry: { path: "b.md", entry: entry("b-only", "Only B") } }], { message: "B" }),
    ])
    if (!(ra.pushed && rb.pushed)) console.log(JSON.stringify({ ra, rb }))
    expect(ra.pushed && rb.pushed).toBe(true)
    const files = await remoteFiles(remote)
    expect(files["a.md"]).toContain("a-only")
    expect(files["b.md"]).toContain("b-only")
    // Exactly one line for the shared id: the later writer's intent re-applied once.
    const all = Object.values(files).join("\n")
    expect(all.split("\n").filter((l) => l.includes("id: same-id")).length).toBe(1)
    // History is linear: no force push ever rewrote it.
    const log = await remoteLog(remote)
    expect(log[log.length - 1]).toBe("Initialize memory drive")
  })

  test("same id rejected push: one retry commit, re-applied on the latest files", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    const a = new DriveCheckout({ dir: path.join(root, "a"), remote: remoteFor(remote), author })
    const b = new DriveCheckout({ dir: path.join(root, "b"), remote: remoteFor(remote), author })
    await a.ensure()
    await b.ensure()
    await a.write([{ UpsertEntry: { path: "notes.md", entry: entry("x", "First text") } }], { message: "A1" })
    // b commits offline-style against its stale head, then pushes.
    await b.write([{ UpsertEntry: { path: "notes.md", entry: entry("x", "Second text") } }], { message: "B1" })
    const notes = (await remoteFiles(remote))["notes.md"]!
    expect(notes).toContain("Second text")
    expect(notes).not.toContain("First text")
  })

  test("concurrent first initialization of one checkout is serialized", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    const dir = path.join(root, "shared")
    const one = new DriveCheckout({ dir, remote: remoteFor(remote), author })
    const two = new DriveCheckout({ dir, remote: remoteFor(remote), author })
    const [r1, r2] = await Promise.all([
      one.write([{ UpsertEntry: { path: "notes.md", entry: entry("one", "From one") } }], { message: "one" }),
      two.write([{ UpsertEntry: { path: "notes.md", entry: entry("two", "From two") } }], { message: "two" }),
    ])
    expect(r1.pushed && r2.pushed).toBe(true)
    const notes = (await remoteFiles(remote))["notes.md"]!
    expect(notes).toContain("id: one")
    expect(notes).toContain("id: two")
    // No leftover temp/lock directories next to the checkout.
    const siblings = await readdir(root)
    expect(siblings.filter((s) => s.includes(".init-") || s.endsWith(".lock"))).toEqual([])
  })

  test("offline write keeps the local commit, shows pending, and the next session pushes it", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    const dir = path.join(root, "a")
    const online = new DriveCheckout({ dir, remote: remoteFor(remote), author })
    await online.ensure()
    const missing = path.join(root, "gone.git")
    const offline = new DriveCheckout({ dir, remote: remoteFor(missing), author })
    const r = await offline.write([{ UpsertEntry: { path: "notes.md", entry: entry("off-1", "Saved while offline") } }], { message: "offline" })
    expect(r.pushed).toBe(false)
    expect(r.pending).toBe(true)
    expect(r.error).toBeTruthy()
    const status = await offline.status()
    expect(status.pending).toBe(true)
    expect(status.lastError).toBeTruthy()
    expect(await readFile(path.join(dir, "notes.md"), "utf8")).toContain("off-1")
    // Meanwhile another device wrote to the server.
    const other = new DriveCheckout({ dir: path.join(root, "b"), remote: remoteFor(remote), author })
    await other.write([{ UpsertEntry: { path: "notes.md", entry: entry("srv-1", "Written elsewhere") } }], { message: "elsewhere" })
    // Next session (remote reachable again) syncs without force.
    const next = new DriveCheckout({ dir, remote: remoteFor(remote), author })
    const after = await next.sync()
    expect(after.pending).toBe(false)
    expect(after.lastError).toBeUndefined()
    const notes = (await remoteFiles(remote))["notes.md"]!
    expect(notes).toContain("off-1")
    expect(notes).toContain("srv-1")
  })

  test("several offline commits keep their own history when they sync", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    const dir = path.join(root, "a")
    await new DriveCheckout({ dir, remote: remoteFor(remote), author }).ensure()
    const offline = new DriveCheckout({ dir, remote: remoteFor(path.join(root, "gone.git")), author })
    await offline.write([{ UpsertEntry: { path: "notes.md", entry: entry("off-1", "First offline note") } }], { message: "Remember: first offline note" })
    await offline.write([{ UpsertEntry: { path: "notes.md", entry: entry("off-2", "Second offline note") } }], { message: "Remember: second offline note" })
    const other = new DriveCheckout({ dir: path.join(root, "b"), remote: remoteFor(remote), author })
    await other.write([{ UpsertEntry: { path: "notes.md", entry: entry("srv-1", "Written elsewhere") } }], { message: "elsewhere" })
    const after = await new DriveCheckout({ dir, remote: remoteFor(remote), author }).sync()
    expect(after.pending).toBe(false)
    const log = await remoteLog(remote)
    expect(log.slice(0, 3)).toEqual(["Remember: second offline note", "Remember: first offline note", "elsewhere"])
    const notes = (await remoteFiles(remote))["notes.md"]!
    for (const id of ["off-1", "off-2", "srv-1"]) expect(notes).toContain(id)
  })

  test("first-login merge brings signed-out history up without force", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    // Server already has memory from the web app.
    const web = new DriveCheckout({ dir: path.join(root, "web"), remote: remoteFor(remote), author })
    await web.write([{ UpsertEntry: { path: "notes.md", entry: entry("web-1", "From the web") } }], { message: "web" })
    const before = (await runGit(["rev-parse", "main"], { cwd: remote })).stdout
    // Signed-out local drive.
    const local = new DriveCheckout({ dir: path.join(root, "local"), author })
    await local.write([{ UpsertEntry: { path: "notes.md", entry: entry("loc-1", "Saved signed out") } }], { message: "local" })
    const account = new DriveCheckout({ dir: path.join(root, "acct"), remote: remoteFor(remote), author })
    const merged = await account.mergeFrom(path.join(root, "local"))
    expect(merged.merged).toBe(true)
    expect(merged.result?.pushed).toBe(true)
    const notes = (await remoteFiles(remote))["notes.md"]!
    expect(notes).toContain("web-1")
    expect(notes).toContain("loc-1")
    const after = await remoteLog(remote)
    // Remote history only grew: the old head is an ancestor of the new one.
    expect((await runGit(["merge-base", "--is-ancestor", before, "main"], { cwd: remote })).code).toBe(0)
    expect(after).toContain("local")
    expect(after[0]).toBe("Merge memory saved while signed out")
    // Idempotent.
    expect((await account.mergeFrom(path.join(root, "local"))).merged).toBe(false)
  })

  test("secrets and malformed sources are blocked before any commit", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    const drive = new DriveCheckout({ dir: path.join(root, "a"), remote: remoteFor(remote), author })
    await drive.ensure()
    const head = await drive.head()
    await expect(
      drive.write([{ UpsertEntry: { path: "notes.md", entry: entry("s1", "deploy key ghp_abcdefghijklmnopqrstuvwxyz0123456789") } }], { message: "x" }),
    ).rejects.toThrow(/credential/)
    await expect(
      drive.write([{ UpsertEntry: { path: "notes.md", entry: entry("s2", "fine", "javascript:alert(1)") } }], { message: "x" }),
    ).rejects.toThrow(/source/)
    await expect(drive.write([{ UpsertEntry: { path: "../escape.md", entry: entry("s3", "x") } }], { message: "x" })).rejects.toThrow(/path/)
    await expect(drive.write([{ UpsertEntry: { path: "session-log.md", entry: entry("s4", "x") } }], { message: "x" })).rejects.toThrow(/transcripts/)
    expect(await drive.head()).toBe(head)
  })

  test("hand edits are committed; invalid ones are moved aside, not lost", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    const dir = path.join(root, "a")
    const drive = new DriveCheckout({ dir, remote: remoteFor(remote), author, defaultSource: "gizzi:session/ses_hand" })
    await drive.ensure()
    await writeFile(path.join(dir, "people.md"), "# People\n\n- Dana runs ops\n")
    await writeFile(path.join(dir, "bad.md"), "# Bad\n\n- Our password is hunter2\n")
    const r = await drive.commitWorkingTree({ message: "Save memory edits" })
    expect(r.changed).toBe(true)
    expect(r.pushed).toBe(true)
    expect(r.error).toMatch(/bad\.md/)
    const files = await remoteFiles(remote)
    expect(files["people.md"]).toContain("- Dana runs ops [source: gizzi:session/ses_hand; added: ")
    expect(files["bad.md"]).toBeUndefined()
    expect(existsSync(path.join(dir, "bad.md"))).toBe(false)
    expect(r.quarantined.length).toBe(1)
    const kept = await readFile(path.join(`${dir}.rejected`, r.quarantined[0]!), "utf8")
    expect(kept).toContain("hunter2")
    const status = await drive.status()
    expect(status.rejected).toMatch(/not saved/)
    expect(status.rejected).not.toContain("hunter2")
    await drive.dismissRejected()
    expect((await drive.status()).rejected).toBeUndefined()
  })

  test("delete entry and read paths (history, search, file list)", async () => {
    const root = await scratch()
    const drive = new DriveCheckout({ dir: path.join(root, "local"), author })
    await drive.write([{ UpsertEntry: { path: "notes.md", entry: entry("d1", "Delete me later") } }, { UpsertEntry: { path: "notes.md", entry: entry("d2", "Keep me") } }], { message: "add" })
    await drive.write([{ DeleteEntry: { id: "d1" } }], { message: "forget" })
    const notes = await drive.readFile("notes.md")
    expect(notes).not.toContain("d1")
    expect(notes).toContain("d2")
    const history = await drive.history(5)
    expect(history.map((c) => c.message)).toEqual(["forget", "add", "Initialize memory drive"])
    expect((await drive.search("keep")).map((h) => h.entry?.id)).toEqual(["d2"])
    expect((await drive.listFiles()).map((f) => f.path)).toEqual(["MEMORY.md", "notes.md"])
    await expect(drive.readFile("../etc/passwd.md")).rejects.toThrow(/path/)
  })

  test("server refusal: reason surfaced, local commit kept, no retry loop", async () => {
    const root = await scratch()
    const remote = await bareRemote(root)
    const a = new DriveCheckout({ dir: path.join(root, "a"), remote: remoteFor(remote), author })
    await a.ensure()
    const hook = path.join(remote, "hooks", "pre-receive")
    await writeFile(hook, '#!/bin/sh\necho "Memory Drive refused this push: notes.md line 1 needs source and added" >&2\nexit 1\n', { mode: 0o755 })
    const r = await a.write([{ UpsertEntry: { path: "notes.md", entry: entry("r1", "Refused fact") } }], { message: "refused" })
    expect(r.pushed).toBe(false)
    expect(r.pending).toBe(true)
    expect(r.error).toBe("The memory server refused the change: notes.md line 1 needs source and added")
    expect(await a.readFile("notes.md")).toContain("r1")
  })

  test("managed folders (twin/, cowork/) are never written", async () => {
    const root = await scratch()
    const drive = new DriveCheckout({ dir: path.join(root, "local"), author })
    await expect(drive.write([{ UpsertEntry: { path: "twin/persona.md", entry: entry("t1", "x") } }], { message: "x" })).rejects.toThrow(/managed/)
    await mkdir(path.join(root, "local", "cowork"), { recursive: true })
    await writeFile(path.join(root, "local", "cowork", "bot.md"), "# Bot\n\n- hand edit\n")
    const r = await drive.commitWorkingTree()
    expect(r.changed).toBe(false)
    expect(r.error).toMatch(/managed/)
  })

  test("external standard bullets without id keep a derived identity", async () => {
    const line = "- Uses tabs [source: claude-code:session/abc; added: 2026-10-01]"
    const parsed = parseEntry(line)
    expect(parsed.id).toBe(deriveEntryId("Uses tabs", "claude-code:session/abc", "2026-10-01"))
    expect(rustLines("a\nb\n")).toEqual(["a", "b"])
  })
})
