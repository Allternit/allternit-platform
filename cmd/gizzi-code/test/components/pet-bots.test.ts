import { afterAll, beforeEach, describe, expect, test } from "bun:test"
import { mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

const dir = mkdtempSync(join(tmpdir(), "pet-bots-"))
const file = join(dir, "desktop-companion.json")
process.env.GIZZI_DESKTOP_PET_FILE = file

const { parsePetBot, GIZZI_BOT_ID, resetPetBotsForTests, selectPetBot, getCurrentPetBot } = await import(
  "../../src/cli/ui/ink-app/pet/petBots"
)
const { readDesktopPetAgentId, writeDesktopPetAgentId, watchDesktopPetAgentId } = await import("../../src/cli/ui/ink-app/pet/desktopPet")
const { petBotRows, PET_SPRITE_WIDTH, eyeColorOn } = await import("../../src/cli/ui/ink-app/pet/botSprite")

afterAll(() => rmSync(dir, { recursive: true, force: true }))
beforeEach(() => resetPetBotsForTests())

const geometric = {
  id: "bot-1",
  name: "scout",
  isBot: true,
  provider: "anthropic",
  model: "claude-sonnet-5",
  botProfile: {
    displayName: "Scout",
    tagline: "research",
    accentColor: "#4FA3A5",
    avatar: {
      type: "geometric",
      data: { seed: "s", shape: "hex", primaryColor: "#4FA3A5", secondaryColor: "#27494A", eyePreset: "wide" },
    },
  },
}

describe("pet bots", () => {
  test("parses an API bot with its avatar, accent and model", () => {
    const bot = parsePetBot(geometric)!
    expect(bot).toMatchObject({ id: "bot-1", name: "Scout", description: "research", accent: "#4FA3A5" })
    expect(bot.avatar).toEqual({ kind: "geometric", shape: "hex", primary: "#4FA3A5", secondary: "#27494A", eyes: "wide" })
    expect(bot.model).toEqual({ providerID: "anthropic", modelID: "claude-sonnet-5" })
  })

  test("skips non-bot agents; image avatars keep their URL, mascots fall back to Gizzi", () => {
    expect(parsePetBot({ id: "a", name: "Helper", isBot: false })).toBeUndefined()
    const image = parsePetBot({ id: "b", name: "Pic", is_bot: 1, botProfile: { avatar: { type: "image", data: { url: "https://x/y.png" } } } })
    expect(image?.avatar).toEqual({ kind: "image", url: "https://x/y.png" })
    const legacy = parsePetBot({ id: "c", name: "Old", isBot: true, config: { avatar: { type: "image", uri: "data:image/png;base64,AAAA" } } })
    expect(legacy?.avatar).toEqual({ kind: "image", url: "data:image/png;base64,AAAA" })
    const mascot = parsePetBot({ id: "d", name: "M", isBot: true, config: { avatar: { type: "mascot", mascot: { template: "cyber" } } } })
    expect(mascot?.avatar).toEqual({ kind: "gizzi" })
    const badUrl = parsePetBot({ id: "e", name: "F", isBot: true, botProfile: { avatar: { type: "image", data: { url: "file:///etc/passwd" } } } })
    expect(badUrl?.avatar).toEqual({ kind: "gizzi" })
    expect(parsePetBot({ id: GIZZI_BOT_ID, name: "gizzi" })?.avatar).toEqual({ kind: "gizzi" })
  })

  test("an image avatar draws as Gizzi where the terminal can't show images", () => {
    const image = parsePetBot({ id: "b", name: "Pic", isBot: true, botProfile: { avatar: { type: "image", data: { url: "https://x/y.png" } } } })!
    const gizzi = parsePetBot({ id: GIZZI_BOT_ID, name: "gizzi" })!
    expect(petBotRows(image, "idle")).toEqual(petBotRows(gizzi, "idle"))
  })

  test("reads a stringified avatar and rejects bad colors", () => {
    const bot = parsePetBot({
      id: "c",
      name: "Kit",
      isBot: true,
      botProfile: { avatar: JSON.stringify({ type: "pet", data: { species: "fox", primaryColor: "red", secondaryColor: "#112233" } }) },
    })!
    expect(bot.avatar).toMatchObject({ kind: "pet", species: "fox", secondary: "#112233", accessory: "none" })
    expect((bot.avatar as { primary: string }).primary).toMatch(/^#[0-9A-F]{6}$/i)
  })
})

describe("desktop pet sync", () => {
  test("reads and writes agentId, keeping Desktop's other settings", () => {
    writeFileSync(file, JSON.stringify({ enabled: true, size: 74, agentId: "gizzi-packaged-assistant", position: { x: 1, y: 2 } }))
    expect(readDesktopPetAgentId()).toBe("gizzi-packaged-assistant")
    expect(writeDesktopPetAgentId("bot-1")).toBe(true)
    expect(JSON.parse(readFileSync(file, "utf8"))).toEqual({ enabled: true, size: 74, agentId: "bot-1", position: { x: 1, y: 2 } })
  })

  test("follows Desktop replacing its settings file through a temp file", async () => {
    writeFileSync(file, JSON.stringify({ agentId: "bot-a" }))
    const seen: Array<string | undefined> = []
    const stop = watchDesktopPetAgentId(id => seen.push(id))
    try {
      await new Promise(r => setTimeout(r, 50))
      writeFileSync(`${file}.tmp`, JSON.stringify({ agentId: "bot-b", size: 80 }))
      renameSync(`${file}.tmp`, file)
      for (let i = 0; i < 40 && seen.length === 0; i++) await new Promise(r => setTimeout(r, 50))
      expect(seen).toEqual(["bot-b"])
    } finally {
      stop()
    }
  })

  test("selecting a bot updates the pet and the Desktop pet", () => {
    writeFileSync(file, JSON.stringify({ agentId: GIZZI_BOT_ID }))
    selectPetBot("bot-9")
    expect(readDesktopPetAgentId()).toBe("bot-9")
    // Unknown until the roster loads: the pet shows Gizzi rather than nothing.
    expect(getCurrentPetBot().id).toBe(GIZZI_BOT_ID)
  })
})

describe("pet sprites", () => {
  const shapes = ["circle", "rounded", "square", "hex", "diamond"] as const
  const species = ["cat", "dog", "rabbit", "fox", "owl", "robot"] as const
  const width = (row: Array<[string, ...unknown[]]>) => row.reduce((n, [text]) => n + text.length, 0)

  test("every shape and species fills the 12x5 pet slot", () => {
    const bots = [
      ...shapes.map(shape => ({ kind: "geometric" as const, shape, primary: "#4FA3A5", secondary: "#27494A", eyes: "round" as const })),
      ...species.map(s => ({ kind: "pet" as const, species: s, primary: "#D4B08C", secondary: "#8A6A4F", accessory: "glasses" as const })),
    ]
    for (const avatar of bots) {
      const rows = petBotRows({ id: "x", name: "X", description: "", avatar }, "idle")
      expect(rows).toHaveLength(5)
      for (const row of rows) expect(width(row as never)).toBe(PET_SPRITE_WIDTH)
    }
  })

  test("blink and wink close eyes; glance moves them", () => {
    const bot = { id: "x", name: "X", description: "", avatar: { kind: "geometric" as const, shape: "circle" as const, primary: "#4FA3A5", secondary: "#27494A", eyes: "round" as const } }
    const eye = eyeColorOn("#27494A")
    // Count half-block pixels drawn in the eye color.
    const eyeCells = (p: "idle" | "blink" | "wink" | "glance") => {
      let n = 0
      for (const row of petBotRows(bot, p)) {
        for (const [text, fg, bg] of row as Array<[string, string?, string?]>) {
          for (const ch of text) {
            if (ch === "█") n += fg === eye ? 2 : 0
            else if (ch === "▀") n += (fg === eye ? 1 : 0) + (bg === eye ? 1 : 0)
            else if (ch === "▄") n += fg === eye ? 1 : 0
          }
        }
      }
      return n
    }
    expect(eyeCells("blink")).toBeLessThan(eyeCells("idle"))
    expect(eyeCells("wink")).toBeLessThan(eyeCells("idle"))
    expect(JSON.stringify(petBotRows(bot, "glance"))).not.toBe(JSON.stringify(petBotRows(bot, "idle")))
  })

  test("eyes contrast with the face", () => {
    expect(eyeColorOn("#101010")).not.toBe(eyeColorOn("#F0F0F0"))
  })
})
