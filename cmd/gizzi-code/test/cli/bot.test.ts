// @ts-nocheck
import { afterAll, beforeAll, describe, expect, test } from "bun:test"
import { existsSync, readFileSync, rmSync } from "node:fs"
import { join } from "node:path"
import { tmpdir } from "../fixture/fixture"

// test/cli/bot.test.ts → repo root is two levels up
const REPO_ROOT = join(import.meta.dir, "..", "..")

interface RunResult {
  code: number
  /** stdout + stderr merged — the CLI prints user-facing output on stderr. */
  output: string
  stdout: string
  stderr: string
}

let configDir: string
let cronDbPath: string

async function runBot(args: string[]): Promise<RunResult> {
  const proc = Bun.spawn(["bun", "src/cli/main.ts", "agents", "bot", ...args], {
    cwd: REPO_ROOT,
    stdout: "pipe",
    stderr: "pipe",
    stdin: "ignore",
    env: {
      ...process.env,
      GIZZI_CONFIG_DIR: configDir,
      GIZZI_CRON_DB_PATH: cronDbPath,
      GIZZI_TELEMETRY: "off",
    },
  })
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ])
  return { code, output: stdout + stderr, stdout, stderr }
}

beforeAll(async () => {
  const tmp = await tmpdir()
  configDir = join(tmp.path, ".gizzi")
  cronDbPath = join(tmp.path, "cron.db")
  // warmup: prime bun's transpile cache so individual spawns stay fast
  await runBot(["list"])
})

afterAll(() => {
  rmSync(configDir, { recursive: true, force: true })
})

describe("gizzi agents bot (phase B1)", () => {
  test("create -- writes bot.json, SOUL.md, memory/ under GIZZI_CONFIG_DIR", { timeout: 60000 }, async () => {
    const res = await runBot([
      "create",
      "cli-bot",
      "--title",
      "CLI Bot",
      "--description",
      "created from a test",
      "--model",
      "anthropic/claude-sonnet-4-5",
    ])
    expect(res.code).toBe(0)
    expect(res.output).toContain("cli-bot")

    const dir = join(configDir, "bots", "cli-bot")
    expect(existsSync(join(dir, "bot.json"))).toBe(true)
    expect(existsSync(join(dir, "SOUL.md"))).toBe(true)
    expect(existsSync(join(dir, "memory"))).toBe(true)

    const bot = JSON.parse(readFileSync(join(dir, "bot.json"), "utf8"))
    expect(bot.name).toBe("cli-bot")
    expect(bot.title).toBe("CLI Bot")
    expect(bot.model).toBe("anthropic/claude-sonnet-4-5")
  })

  test("create --soul seeds SOUL.md from a file", { timeout: 60000 }, async () => {
    const soulPath = join(configDir, "test-soul.md")
    await Bun.write(soulPath, "# SOUL\nTest persona.\n")
    const res = await runBot(["create", "souled", "--title", "Souled", "--soul", soulPath])
    expect(res.code).toBe(0)
    expect(readFileSync(join(configDir, "bots", "souled", "SOUL.md"), "utf8")).toBe(
      "# SOUL\nTest persona.\n",
    )
  })

  test("create -- rejects bad slugs and reserved names", { timeout: 60000 }, async () => {
    for (const name of ["Bad_Name", "list"]) {
      const res = await runBot(["create", name])
      expect(res.code).toBe(1)
    }
  })

  test("list -- shows the created bots with canonical chat status", { timeout: 60000 }, async () => {
    const res = await runBot(["list"])
    expect(res.code).toBe(0)
    expect(res.output).toContain("NAME")
    expect(res.output).toContain("cli-bot")
    expect(res.output).toContain("CLI Bot")
    expect(res.output).toContain("anthropic/claude-sonnet-4-5")
    // no canonical session pinned yet
    expect(res.output).not.toContain("pinned:")
  })

  test("show -- prints identity, SOUL preview, and memory count", { timeout: 60000 }, async () => {
    const res = await runBot(["show", "cli-bot"])
    expect(res.code).toBe(0)
    expect(res.output).toContain("cli-bot")
    expect(res.output).toContain("CLI Bot")
    expect(res.output).toContain("Memory notes:")
    expect(res.output).toContain("SOUL.md")
    expect(res.output).toContain("not pinned")
  })

  test("show/edit/delete -- unknown name exits 1", { timeout: 60000 }, async () => {
    for (const cmd of [
      ["show", "ghost"],
      ["edit", "ghost", "--title", "x"],
      ["delete", "ghost", "--yes"],
    ]) {
      const res = await runBot(cmd)
      expect(res.code).toBe(1)
      expect(res.output).toContain("not found")
    }
  })

  test("edit -- patches identity and bumps updatedAt", { timeout: 60000 }, async () => {
    const before = JSON.parse(
      readFileSync(join(configDir, "bots", "cli-bot", "bot.json"), "utf8"),
    )
    const res = await runBot(["edit", "cli-bot", "--title", "Renamed Bot", "--model", ""])
    expect(res.code).toBe(0)
    const after = JSON.parse(
      readFileSync(join(configDir, "bots", "cli-bot", "bot.json"), "utf8"),
    )
    expect(after.title).toBe("Renamed Bot")
    expect(after.model).toBeNull()
    expect(after.updatedAt >= before.updatedAt).toBe(true)
    // show reflects the edit (case-insensitive lookup)
    const shown = await runBot(["show", "CLI-BOT"])
    expect(shown.code).toBe(0)
    expect(shown.output).toContain("Renamed Bot")
  })

  test("clone -- copies the profile into a new bot", { timeout: 60000 }, async () => {
    const res = await runBot(["clone", "cli-bot", "cli-bot-2"])
    expect(res.code).toBe(0)
    expect(res.output).toContain("cli-bot-2")
    expect(existsSync(join(configDir, "bots", "cli-bot-2", "SOUL.md"))).toBe(true)

    const list = await runBot(["list"])
    expect(list.output).toContain("cli-bot-2")
    // neither source nor clone has a pinned canonical chat
    expect(list.output).not.toContain("pinned:")
  })

  test("chat -- unknown bot exits 1", { timeout: 60000 }, async () => {
    const res = await runBot(["chat", "ghost"])
    expect(res.code).toBe(1)
    expect(res.output).toContain("not found")
  })

  test("chat -- non-interactive without a message prints a usage hint and exits 1", { timeout: 60000 }, async () => {
    const res = await runBot(["chat", "cli-bot"])
    expect(res.code).toBe(1)
    expect(res.output).toContain("usage: gizzi agents bot chat <name> [message..]")
    // nothing was pinned — the chat never opened
    const bot = JSON.parse(readFileSync(join(configDir, "bots", "cli-bot", "bot.json"), "utf8"))
    expect(bot.canonicalSession).toBeNull()
  })

  test("delete -- removes the bot home and prints what was removed", { timeout: 60000 }, async () => {
    const res = await runBot(["delete", "cli-bot", "--yes"])
    expect(res.code).toBe(0)
    expect(res.output).toContain("Deleted bot 'cli-bot'")
    expect(existsSync(join(configDir, "bots", "cli-bot"))).toBe(false)
  })

  test("delete -- without --yes refuses in a non-interactive shell and deletes nothing", { timeout: 60000 }, async () => {
    const res = await runBot(["delete", "souled"])
    expect(res.code).toBe(1)
    expect(res.output).toContain("--yes")
    // nothing was deleted
    expect(existsSync(join(configDir, "bots", "souled"))).toBe(true)
    // clean up for a tidy temp dir
    const cleanup = await runBot(["delete", "souled", "--yes"])
    expect(cleanup.code).toBe(0)
  })
})

describe("gizzi agents bot routine (phase B3)", () => {
  test("add -- creates a [bot:<name>] agent job with config.bot", { timeout: 60000 }, async () => {
    const created = await runBot(["create", "routine-bot", "--title", "Routine Bot"])
    expect(created.code).toBe(0)
    const res = await runBot([
      "routine",
      "add",
      "routine-bot",
      "--schedule",
      "daily at 9am",
      "--prompt",
      "summarize the inbox",
    ])
    expect(res.code).toBe(0)
    expect(res.output).toContain("[bot:routine-bot] summarize the inbox")
    expect(res.output).toContain("canonical chat")
    expect(res.output).toContain("Schedule:")
    expect(res.output).toContain("9:00")
  })

  test("add -- unknown bot exits 1", { timeout: 60000 }, async () => {
    const res = await runBot([
      "routine",
      "add",
      "ghost",
      "--schedule",
      "daily",
      "--prompt",
      "x",
    ])
    expect(res.code).toBe(1)
    expect(res.output).toContain("not found")
  })

  test("add -- missing --schedule/--prompt fails argument validation", { timeout: 60000 }, async () => {
    const missingSchedule = await runBot(["routine", "add", "routine-bot", "--prompt", "x"])
    expect(missingSchedule.code).toBe(1)
    const missingPrompt = await runBot(["routine", "add", "routine-bot", "--schedule", "daily"])
    expect(missingPrompt.code).toBe(1)
  })

  test("list -- shows schedule, status, and next run for the bot's routines", { timeout: 60000 }, async () => {
    const listed = await runBot(["routine", "list", "routine-bot"])
    expect(listed.code).toBe(0)
    expect(listed.output).toContain("summarize the inbox")
    expect(listed.output).toContain("NEXT RUN")
    // no routines on a fresh bot
    await runBot(["create", "empty-bot", "--title", "Empty"])
    const empty = await runBot(["routine", "list", "empty-bot"])
    expect(empty.code).toBe(0)
    expect(empty.output).toContain("No routines for bot 'empty-bot'")
  })

  test("list -- unknown bot exits 1", { timeout: 60000 }, async () => {
    const res = await runBot(["routine", "list", "ghost"])
    expect(res.code).toBe(1)
    expect(res.output).toContain("not found")
  })

  test("remove -- prefix enforcement and round trip", { timeout: 60000 }, async () => {
    // grab the job id from `bot routine list` output via `cron list`-style name
    const listed = await runBot(["routine", "list", "routine-bot"])
    expect(listed.code).toBe(0)

    // find the job id directly in the cron db — parse it with bun:sqlite
    const { Database } = await import("bun:sqlite")
    const db = new Database(cronDbPath, { readonly: true })
    const row = db
      .query("SELECT id, name FROM jobs WHERE name LIKE '[bot:routine-bot]%' LIMIT 1")
      .get() as { id: string; name: string } | null
    db.close()
    expect(row).not.toBeNull()

    // another bot cannot remove it
    const foreign = await runBot(["routine", "remove", "empty-bot", row!.id])
    expect(foreign.code).toBe(1)
    expect(foreign.output).toContain("does not belong")

    // the owner removes it
    const removed = await runBot(["routine", "remove", "routine-bot", row!.id])
    expect(removed.code).toBe(0)
    expect(removed.output).toContain("Removed routine")

    // gone: list is empty, removing again fails
    const after = await runBot(["routine", "list", "routine-bot"])
    expect(after.output).toContain("No routines")
    const again = await runBot(["routine", "remove", "routine-bot", row!.id])
    expect(again.code).toBe(1)
    expect(again.output).toContain("not found")
  })
})
