import { afterAll, describe, expect, test } from "bun:test"
import { mkdir, mkdtemp, readFile, rm, utimes, writeFile } from "fs/promises"
import os from "os"
import path from "path"
import { DriveCheckout } from "../../../src/runtime/memory/drive/checkout"
import { applyMemdirImport, planMemdirImport, RECEIPT_PATH, sanitizeText } from "../../../src/runtime/memory/drive/import"
import { collectEntries } from "../../../src/runtime/memory/drive/format"

const roots: string[] = []
afterAll(async () => {
  for (const r of roots) await rm(r, { recursive: true, force: true })
})

const fm = (name: string, description: string, type: string, body: string) =>
  ["---", `name: ${name}`, `description: ${description}`, `type: ${type}`, "---", "", body].join("\n")

async function legacyTree(): Promise<{ root: string; projects: string }> {
  const root = await mkdtemp(path.join(os.tmpdir(), "gizzi-import-"))
  roots.push(root)
  const projects = path.join(root, "projects")
  const memdir = path.join(projects, "-Users-eoj-allternit-ai", "memory")
  await mkdir(memdir, { recursive: true })
  await writeFile(path.join(memdir, "MEMORY.md"), "# Memory Index\n\n- [user_role](user_role.md) — role (user)\n")
  await writeFile(path.join(memdir, "user_role.md"), fm("user_role", "The user's role", "user", "Founder of Allternit; prefers terse answers.\n\n**Why:** said so -> often"))
  await writeFile(path.join(memdir, "testing.md"), fm("testing_feedback", "How to test", "feedback", "Run bun test with --preload."))
  await writeFile(path.join(memdir, "creds.md"), fm("deploy", "Deploy notes", "project", "token: ghp_abcdefghijklmnopqrstuvwxyz012345"))
  await writeFile(path.join(memdir, "plain.md"), "just some text without frontmatter\n")
  const when = new Date("2026-03-04T12:00:00Z")
  await utimes(path.join(memdir, "user_role.md"), when, when)
  return { root, projects }
}

describe("legacy memdir import", () => {
  test("dry run plans without writing; apply is one idempotent commit with honest provenance", async () => {
    const { root, projects } = await legacyTree()
    const drive = new DriveCheckout({ dir: path.join(root, "drive"), author: { name: "t", email: "t@t" } })
    const plan = await planMemdirImport(drive, [projects])
    expect(plan.total).toBe(5)
    expect(plan.converted).toBe(2)
    expect(plan.skipped).toBe(3)
    expect(plan.already_imported).toBe(false)
    expect(plan.rows.find((r) => r.file.endsWith("creds.md"))?.reason).toMatch(/credential/)
    expect(plan.topic_files).toEqual(["preferences.md", "user.md"])
    const head = await drive.head()
    expect((await drive.history()).length).toBe(1) // dry run wrote nothing

    const applied = await applyMemdirImport(drive, [projects])
    expect(applied.applied).toBe(true)
    const history = await drive.history()
    expect(history[0]!.message).toBe("Import existing gizzi memory")
    expect(history[1]!.revision).toBe(head!)
    const user = await drive.readFile("user.md")
    const entries = [...collectEntries({ "user.md": user }).values()]
    expect(entries).toHaveLength(1)
    const e = entries[0]!.entry
    expect(e.source).toBe("imported:unknown")
    expect(e.added).toBe("2026-03-04")
    expect(e.text).toContain("user_role: The user's role — Founder of Allternit; prefers terse answers.")
    expect(e.text).toContain("said so -› often")
    expect(e.metadata.origin).toBe("gizzi-memdir")
    expect(e.metadata.memory_type).toBe("fact")
    expect(await drive.readFile(RECEIPT_PATH)).toMatch(/^# Import: gizzi memdir\n\n## /)
    // Sources untouched.
    expect(await readFile(path.join(projects, "-Users-eoj-allternit-ai", "memory", "creds.md"), "utf8")).toContain("ghp_")

    // Idempotent: plan reports it, apply is a no-op.
    expect((await planMemdirImport(drive, [projects])).already_imported).toBe(true)
    const again = await applyMemdirImport(drive, [projects])
    expect(again.applied).toBe(false)
    expect((await drive.history())[0]!.message).toBe("Import existing gizzi memory")
  })

  test("sanitizer keeps words but fits the one-line format", () => {
    expect(sanitizeText("a <b> ![img](x) [note](../file.md) ok [key: v]")).toBe("a ‹b› ! [img] (x) [note] (../file.md) ok (key: v]")
  })
})
