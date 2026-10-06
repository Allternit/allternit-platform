import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test"
import { existsSync } from "fs"
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "fs/promises"
import os from "os"
import path from "path"
import { MemoryDrive, sessionSource, topicPath } from "../../../src/runtime/memory/drive/drive"
import { driveCheckoutPath, localDriveCheckoutPath, resetDriveAccountCache, serverDreamingActiveSync } from "../../../src/runtime/memory/drive/paths"
import { runGit } from "../../../src/runtime/memory/drive/git"
import { formatOverview, formatSearch, syncLine } from "../../../src/runtime/memory/drive/report"

const roots: string[] = []
afterAll(async () => {
  for (const r of roots) await rm(r, { recursive: true, force: true })
})

const realFetch = globalThis.fetch
let root = ""

async function bare(dir: string): Promise<string> {
  await mkdir(dir, { recursive: true })
  await runGit(["init", "--bare", "--quiet"], { cwd: dir })
  await runGit(["symbolic-ref", "HEAD", "refs/heads/main"], { cwd: dir })
  return dir
}

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } })

interface Server {
  calls: { method: string; url: string; body: any; auth: string | null }[]
  remotes: Record<string, string>
  mounts: { ref: string; kind: string; name: string; access: "read" | "write" }[]
  dreaming: boolean
}

function mockPlatform(server: Server) {
  globalThis.fetch = (async (input: string | URL, init: RequestInit = {}) => {
    const url = new URL(String(input))
    const body = init.body ? JSON.parse(String(init.body)) : undefined
    const auth = new Headers(init.headers).get("Authorization")
    server.calls.push({ method: init.method ?? "GET", url: url.pathname + url.search, body, auth })
    const ref = url.searchParams.get("drive") ?? body?.drive ?? "personal"
    if (url.pathname.endsWith("/memory/drive/info")) {
      return json({ ref, name: "memory", brain_id: `b-${ref}`, clone_url: server.remotes[ref], branch: "main" })
    }
    if (url.pathname.endsWith("/memory/drive/tokens")) {
      return json({ id: `tok-${server.calls.length}`, token: `drive-token-${ref}`, username: "x-access-token", clone_url: server.remotes[ref], access: body.access })
    }
    if (url.pathname.endsWith("/memory/drive/mounts")) return json({ mounts: server.mounts })
    if (url.pathname.endsWith("/memory/drive/settings")) return json({ dreaming_enabled: server.dreaming })
    if (url.pathname.endsWith("/memory/drive/dreams")) {
      return json({ dreams: [{ id: "d1", date: "2026-10-06", status: "applied", revision: "a".repeat(40), summary: { merged: 2, resolved: 0, lessons: 1, pruned: 0, proposals: 0 } }] })
    }
    return json({ error: "not found" }, 404)
  }) as typeof fetch
}

beforeEach(async () => {
  root = await mkdtemp(path.join(os.tmpdir(), "gizzi-drive-acct-"))
  roots.push(root)
  process.env.GIZZI_MEMORY_DRIVE_ROOT = path.join(root, "drives")
  delete process.env.ALLTERNIT_API_TOKEN
  delete process.env.GIZZI_MEMORY_DRIVE
  resetDriveAccountCache()
  MemoryDrive.resetForTests()
})

afterEach(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_API_TOKEN
  delete process.env.GIZZI_MEMORY_DRIVE_ROOT
  resetDriveAccountCache()
  MemoryDrive.resetForTests()
})

function signIn(token: string) {
  process.env.ALLTERNIT_API_TOKEN = token
  resetDriveAccountCache()
  MemoryDrive.resetForTests()
}

function signOut() {
  delete process.env.ALLTERNIT_API_TOKEN
  resetDriveAccountCache()
  MemoryDrive.resetForTests()
}

describe("MemoryDrive accounts, sync and session context", () => {
  test("signed out: local-only drive, remember writes a spec bullet with the session source", async () => {
    const saved = await MemoryDrive.remember({ text: "Prefers  tabs\nover spaces", type: "feedback", sessionId: "ses_abc" })
    expect(saved.path).toBe("preferences.md")
    expect(saved.result.pushed).toBe(false)
    expect(saved.result.pending).toBe(false)
    expect(saved.entry.source).toBe("gizzi:session/ses_abc")
    expect(saved.entry.id).toMatch(/^entry-[0-9a-f]{64}$/)
    const dir = localDriveCheckoutPath()
    const content = await readFile(path.join(dir, "preferences.md"), "utf8")
    expect(content).toContain(`- Prefers tabs over spaces [source: gizzi:session/ses_abc; added: `)
    expect(content).toContain("memory_type: preference")
    expect(await readFile(path.join(dir, "MEMORY.md"), "utf8")).toContain("- [[preferences]]")
    // Update by id keeps the location; delete removes it.
    await MemoryDrive.remember({ text: "Prefers tabs everywhere", id: saved.entry.id, sessionId: "ses_abc" })
    const updated = await readFile(path.join(dir, "preferences.md"), "utf8")
    expect(updated).toContain("Prefers tabs everywhere")
    expect(updated).not.toContain("over spaces")
    expect((await MemoryDrive.forget(saved.entry.id)).found).toBe(true)
    expect(await readFile(path.join(dir, "preferences.md"), "utf8")).not.toContain(saved.entry.id)
  })

  test("account isolation: each account and the signed-out drive get their own checkout", async () => {
    const remoteA = await bare(path.join(root, "a.git"))
    const remoteB = await bare(path.join(root, "b.git"))
    const local = localDriveCheckoutPath()
    signIn("token-a")
    const dirA = driveCheckoutPath()
    signIn("token-b")
    const dirB = driveCheckoutPath()
    expect(new Set([local, dirA, dirB]).size).toBe(3)

    // Signed out: write locally.
    signOut()
    await MemoryDrive.remember({ text: "Local note before login", sessionId: "s0" })

    // First sign-in (account A) merges it up, then claims it.
    signIn("token-a")
    const serverA: Server = { calls: [], remotes: { personal: remoteA }, mounts: [], dreaming: false }
    mockPlatform(serverA)
    await MemoryDrive.prepare()
    const filesA = (await runGit(["show", "main:notes.md"], { cwd: remoteA })).stdout
    expect(filesA).toContain("Local note before login")
    const log = (await runGit(["log", "--format=%s", "main"], { cwd: remoteA })).stdout
    expect(log.split("\n")[0]).toBe("Merge memory saved while signed out")
    // Token minted write-scoped with the gizzi label; never stored in the remote URL.
    const mint = serverA.calls.find((c) => c.url.endsWith("/memory/drive/tokens"))!
    expect(mint.body.access).toBe("write")
    expect(mint.body.label).toMatch(/^gizzi on /)
    const originUrl = (await runGit(["remote", "get-url", "origin"], { cwd: dirA })).stdout
    expect(originUrl).toBe(remoteA)
    expect(await readFile(path.join(dirA, ".git", "config"), "utf8")).not.toContain("drive-token")
    const store = await stat(path.join(path.dirname(dirA), "credentials.json"))
    expect(store.mode & 0o077).toBe(0)

    // Account B signs in later: the signed-out drive is not merged into it.
    signIn("token-b")
    mockPlatform({ calls: [], remotes: { personal: remoteB }, mounts: [], dreaming: false })
    await MemoryDrive.prepare()
    const lsB = (await runGit(["ls-tree", "-r", "--name-only", "main"], { cwd: remoteB })).stdout
    expect(lsB).not.toContain("notes.md")
    expect(existsSync(path.join(dirB, "notes.md"))).toBe(false)
  })

  test("session context injects each drive's MEMORY.md with its checkout path and mount label", async () => {
    const personal = await bare(path.join(root, "p.git"))
    const project = await bare(path.join(root, "proj.git"))
    // Seed the project drive from elsewhere.
    const { DriveCheckout } = await import("../../../src/runtime/memory/drive/checkout")
    const seeder = new DriveCheckout({ dir: path.join(root, "seed"), remote: { url: project, token: async () => "x" }, author: { name: "t", email: "t@t" } })
    await seeder.write([{ UpsertEntry: { path: "decisions.md", entry: { id: "dec-1", text: "Ship weekly", source: "/?session=s9", added: "2026-10-01", metadata: {} } } }], { message: "seed" })

    signIn("token-ctx")
    mockPlatform({
      calls: [],
      remotes: { personal, "project:p1": project },
      mounts: [{ ref: "project:p1", kind: "project", name: "Gizzi project", access: "read" }],
      dreaming: true,
    })
    await MemoryDrive.remember({ text: "Eoj lives in Saint Paul", type: "user", sessionId: "s1" })
    const context = await MemoryDrive.sessionContext({ waitMs: 60_000 })
    expect(context).toContain("# Memory Drive")
    expect(context).toContain(`Checkout: \`${driveCheckoutPath()}\``)
    expect(context).toContain("- [[user]]")
    expect(context).toContain("## Gizzi project (project:p1) — read-only")
    expect(context).toContain("- [[decisions]]")
    expect(context).toContain("memory_write")
    // Dreaming setting cached → local autoDream gate closed.
    expect(serverDreamingActiveSync()).toBe(true)
    // Writes to a read-only mount are refused.
    await expect(MemoryDrive.remember({ text: "nope", ref: "project:p1" })).rejects.toThrow(/read-only/)
    // Search covers personal and mounts.
    const hits = await MemoryDrive.search("weekly")
    expect(hits.map((h) => h.ref)).toEqual(["project:p1"])
    expect(formatSearch("weekly", hits)).toContain("project:p1:decisions.md")
  })

  test("TUI/CLI overview strings: files, history and sync state", async () => {
    await MemoryDrive.remember({ text: "Uses bun for scripts", topic: "tools", sessionId: "s2" })
    const overview = await MemoryDrive.overview("personal", 5)
    const text = formatOverview(overview)
    expect(text).toContain("Memory Drive — Personal (local, signed out)")
    expect(text).toContain("Sync: local only (signed out). Run `gizzi login` to sync it to your Allternit account.")
    expect(text).toContain("  tools.md  1 memory")
    expect(text).toContain("  MEMORY.md  (index)")
    expect(text).toMatch(/Remember: Uses bun for scripts/)
    expect(syncLine({ ...overview.status, remote: "x", pending: true, ahead: 2, lastError: "offline" }, true)).toBe(
      "Sync: 2 changes waiting to sync — offline",
    )
  })

  test("drive disabled: GIZZI_MEMORY_DRIVE=0 injects nothing", async () => {
    process.env.GIZZI_MEMORY_DRIVE = "0"
    expect(await MemoryDrive.sessionContext()).toBe("")
    delete process.env.GIZZI_MEMORY_DRIVE
  })

  test("helpers: topic paths and sources stay inside the format", () => {
    expect(topicPath("Projects/Gizzi Code")).toBe("projects/gizzi-code.md")
    expect(topicPath(undefined, "project", "/Users/x/allternit-ai")).toBe("projects/allternit-ai.md")
    expect(topicPath("session")).toBe("notes-session.md")
    expect(topicPath("build-log")).toBe("notes.md")
    expect(sessionSource("ses_1/../x")).toBe("gizzi:session/ses_1..x")
  })
})
