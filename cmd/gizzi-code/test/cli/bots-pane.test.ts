// @ts-nocheck
/**
 * Phase B5 — TUI bots pane: row-list rendering (presence dot, unread badge,
 * empty state), keybinding dispatch, and the Enter action wiring
 * (openCanonicalChat + markBotRead + switchSession).
 *
 * Runtime bots modules are mocked at the seam; roster rows are plain data.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, mock, test } from "bun:test"
import { Writable } from "node:stream"
import { join } from "node:path"
import React from "react"
import { tmpdir } from "../fixture/fixture"

import {
  EMPTY_BOTS_MESSAGE,
  buildBotRowSegments,
  formatBotRowText,
  truncate,
} from "../../src/cli/ui/ink-app/screens/bots-pane/rows"
import { handleBotsPaneKey } from "../../src/cli/ui/ink-app/screens/bots-pane/keys"

/* ------------------------------------------------------------------------ */
/* Row formatting                                                           */
/* ------------------------------------------------------------------------ */

describe("bots pane rows", () => {
  const baseRow = {
    name: "scout",
    title: "Scout",
    description: "",
    model: null,
    hasCanonicalChat: false,
    active: false,
    unreadCount: 0,
  }

  test("presence dot renders only in the active state color", () => {
    expect(buildBotRowSegments({ ...baseRow, active: true }).glyph).toBe("●")
    expect(buildBotRowSegments({ ...baseRow, active: true }).glyphColor).toBe("success")
    expect(buildBotRowSegments(baseRow).glyph).toBe("○")
    expect(buildBotRowSegments(baseRow).glyphColor).toBe("inactive")
  })

  test("unread badge only when unreadCount > 0", () => {
    expect(buildBotRowSegments({ ...baseRow, unreadCount: 3 }).badge).toBe("[3]")
    expect(buildBotRowSegments({ ...baseRow, unreadCount: 1 }).badge).toBe("[1]")
    expect(buildBotRowSegments(baseRow).badge).toBe("")
  })

  test("identity merges name and title, model appended when pinned", () => {
    const s = buildBotRowSegments({
      ...baseRow,
      title: "Night Watch",
      model: "claude-sonnet-5",
    })
    expect(s.identity).toBe("scout — Night Watch")
    expect(s.model).toBe(" · claude-sonnet-5")
  })

  test("description is clipped to the row budget", () => {
    const long = "word ".repeat(30)
    const s = buildBotRowSegments({ ...baseRow, description: long })
    expect(s.description.length).toBeLessThanOrEqual(48)
    expect(s.description.endsWith("…")).toBe(true)
  })

  test("formatBotRowText assembles dot, identity, description, model, badge", () => {
    const text = formatBotRowText({
      ...baseRow,
      title: "Scout",
      description: "watches the fleet",
      model: "gpt-5",
      active: true,
      unreadCount: 2,
    })
    expect(text).toContain("●")
    expect(text).toContain("scout — Scout")
    expect(text).toContain("watches the fleet")
    expect(text).toContain("gpt-5")
    expect(text).toContain("[2]")
  })

  test("empty-state message is exact", () => {
    expect(EMPTY_BOTS_MESSAGE).toBe("No bots yet — press n to create one.")
  })

  test("truncate leaves short text alone and marks clipped text", () => {
    expect(truncate("abc", 10)).toBe("abc")
    expect(truncate("abcdefghij", 10)).toBe("abcdefghij")
    expect(truncate("abcdefghijk", 10)).toBe("abcdefghi…")
  })
})

/* ------------------------------------------------------------------------ */
/* Keybindings                                                              */
/* ------------------------------------------------------------------------ */

function makeHandlers() {
  const fired: string[] = []
  const names = [
    "moveUp",
    "moveDown",
    "openSelected",
    "createNew",
    "deleteSelected",
    "refresh",
    "exit",
  ]
  const handlers = Object.fromEntries(names.map(n => [n, () => fired.push(n)]))
  return { handlers, fired }
}

describe("bots pane key dispatch", () => {
  test("arrows and j/k move", () => {
    const { handlers, fired } = makeHandlers()
    expect(handleBotsPaneKey("", { upArrow: true }, handlers)).toBe(true)
    expect(handleBotsPaneKey("", { downArrow: true }, handlers)).toBe(true)
    expect(handleBotsPaneKey("k", {}, handlers)).toBe(true)
    expect(handleBotsPaneKey("j", {}, handlers)).toBe(true)
    expect(fired).toEqual(["moveUp", "moveDown", "moveUp", "moveDown"])
  })

  test("Enter opens, n creates, d deletes, r refreshes", () => {
    const { handlers, fired } = makeHandlers()
    handleBotsPaneKey("", { return: true }, handlers)
    handleBotsPaneKey("n", {}, handlers)
    handleBotsPaneKey("d", {}, handlers)
    handleBotsPaneKey("r", {}, handlers)
    expect(fired).toEqual(["openSelected", "createNew", "deleteSelected", "refresh"])
  })

  test("q and Esc exit", () => {
    const { handlers, fired } = makeHandlers()
    handleBotsPaneKey("q", {}, handlers)
    handleBotsPaneKey("", { escape: true }, handlers)
    expect(fired).toEqual(["exit", "exit"])
  })

  test("modifier chords and unknown keys are left unclaimed", () => {
    const { handlers, fired } = makeHandlers()
    expect(handleBotsPaneKey("q", { ctrl: true }, handlers)).toBe(false)
    expect(handleBotsPaneKey("n", { meta: true }, handlers)).toBe(false)
    expect(handleBotsPaneKey("x", {}, handlers)).toBe(false)
    expect(handleBotsPaneKey("", {}, handlers)).toBe(false)
    expect(fired).toEqual([])
  })
})

/* ------------------------------------------------------------------------ */
/* Row-list render (mocked roster data, real ink)                           */
/* ------------------------------------------------------------------------ */

const stripAnsi = (s: string) => s.replace(/\x1B\[[0-?]*[ -/]*[@-~]/g, "")

describe("BotsRowList render", () => {
  test("renders rows with presence dot, unread badge, and empty state", async () => {
    const { render } = await import("../../src/cli/ui/ink-app/ink")
    const { BotsRowList } = await import("../../src/cli/ui/ink-app/screens/bots-pane/BotsRowList")
    const rows = [
      {
        name: "scout",
        title: "Scout",
        description: "watches the fleet",
        model: "gpt-5",
        hasCanonicalChat: true,
        active: true,
        unreadCount: 2,
      },
      {
        name: "scribe",
        title: "Scribe",
        description: "",
        model: null,
        hasCanonicalChat: false,
        active: false,
        unreadCount: 0,
      },
    ]
    let frames = ""
    const stdout = new Writable({
      write(chunk, _enc, cb) {
        frames += chunk.toString()
        cb()
      },
    })
    const instance = await render(React.createElement(BotsRowList, { rows, selectedIndex: 0 }), {
      stdout,
      exitOnCtrlC: false,
      patchConsole: false,
    })
    await new Promise(r => setTimeout(r, 150))
    instance.unmount()
    const plain = stripAnsi(frames)
    expect(plain).toContain("scout — Scout")
    expect(plain).toContain("watches the fleet")
    expect(plain).toContain("gpt-5")
    expect(plain).toContain("[2]")
    expect(plain).toContain("scribe — Scribe")

    // Empty state.
    let emptyFrames = ""
    const emptyStdout = new Writable({
      write(chunk, _enc, cb) {
        emptyFrames += chunk.toString()
        cb()
      },
    })
    const emptyInstance = await render(
      React.createElement(BotsRowList, { rows: [], selectedIndex: 0 }),
      { stdout: emptyStdout, exitOnCtrlC: false, patchConsole: false },
    )
    await new Promise(r => setTimeout(r, 150))
    emptyInstance.unmount()
    expect(stripAnsi(emptyFrames)).toContain(EMPTY_BOTS_MESSAGE)
  })
})

/* ------------------------------------------------------------------------ */
/* Enter action: openCanonicalChat → markBotRead → switchSession            */
/* ------------------------------------------------------------------------ */

describe("openBotCanonicalChat", () => {
  let tmp
  const calls = { open: 0, read: [] as string[], switched: [] as unknown[], resumed: [] as unknown[] }

  beforeEach(async () => {
    calls.open = 0
    calls.read = []
    calls.switched = []
    calls.resumed = []
    tmp = await tmpdir()
    process.env.GIZZI_CONFIG_DIR = join(tmp.path, ".gizzi")
  })

  afterEach(() => {
    delete process.env.GIZZI_CONFIG_DIR
  })

  // DI fakes — open-bot-chat.ts takes every collaborator as an optional dep
  // precisely so this file never needs bun's mock.module (its registrations
  // leak process-wide and poison test/runtime/bots/* when co-run).
  function fakeDeps(overrides: Record<string, unknown> = {}) {
    return {
      openCanonicalChat: async (bot: any) => {
        calls.open++
        return { projectPath: "/proj/demo", sessionId: "ses_canonical_1", created: false }
      },
      markBotRead: async (name: string) => {
        calls.read.push(name)
      },
      getResumeHandler: () => undefined,
      switchSession: (id: unknown, projectDir: unknown) => {
        calls.switched.push([id, projectDir])
      },
      getLastSessionLog: async (sessionId: string) => ({ sessionId, messages: [] }),
      isLiteLog: () => false,
      loadFullLog: async (log: unknown) => log,
      resolveHandoffHead: async () => null,
      loadEarlierWindow: async () => null,
      ...overrides,
    }
  }

  test("follows a handoff: opens the newest window seeded with the checkpoint", async () => {
    const { createBot, getBot } = await import("../../src/runtime/bots/bot-store")
    await createBot({ name: "scout", title: "Scout" })
    const { openBotCanonicalChat } = await import(
      "../../src/cli/ui/ink-app/screens/bots-pane/open-bot-chat"
    )
    const result = await openBotCanonicalChat("scout", {
      ...fakeDeps(),
      getBot,
      // Desktop handed the chat off; the REPL has no transcript for the head.
      resolveHandoffHead: async (id: string) =>
        id === "ses_canonical_1"
          ? { sessionId: "ses_head_2", checkpoint: "[checkpoint: window 1] Decided: ship 2.1.4 Friday.", generation: 2, reason: "threshold", at: "2026-09-27T10:02:00Z" }
          : null,
      getLastSessionLog: async () => null,
      getResumeHandler: () => async (sessionId: string, log: any, entrypoint: string) => {
        calls.resumed.push([sessionId, log, entrypoint])
      },
    })
    expect(result.sessionId).toBe("ses_head_2")
    expect(calls.switched).toEqual([])
    const [[sessionId, log, entrypoint]] = calls.resumed as any
    expect([sessionId, entrypoint]).toEqual(["ses_head_2", "bots_pane"])
    // Drawn as the rip (boundary tagged with the handoff) and carried into
    // the model's context as the compact summary.
    expect(log.messages).toHaveLength(2)
    expect(log.messages[0]).toMatchObject({ type: "system", subtype: "compact_boundary", sessionId: "ses_head_2", cwd: "/proj/demo" })
    expect(log.messages[0].compactMetadata.handoff).toEqual({ generation: 2, reason: "threshold" })
    expect(log.messages[1]).toMatchObject({ type: "user", isCompactSummary: true, isVisibleInTranscriptOnly: true })
    expect(JSON.stringify(log.messages[1].message.content)).toContain("ship 2.1.4 Friday")
  })

  test("puts the earlier window's last messages above the rip, out of the model's context", async () => {
    const { createBot, getBot } = await import("../../src/runtime/bots/bot-store")
    await createBot({ name: "scout", title: "Scout" })
    const { openBotCanonicalChat } = await import(
      "../../src/cli/ui/ink-app/screens/bots-pane/open-bot-chat"
    )
    const { getMessagesAfterCompactBoundary } = await import("../../src/cli/ui/ink-app/utils/messages")
    await openBotCanonicalChat("scout", {
      ...fakeDeps(),
      getBot,
      resolveHandoffHead: async () => ({ sessionId: "ses_head_3", from: "ses_gen2", checkpoint: "Decided: ship Friday.", generation: 3, reason: "threshold", at: "2026-09-28T10:00:00Z" }),
      loadEarlierWindow: async (head: any) => {
        expect(head.from).toBe("ses_gen2")
        return {
          total: 60,
          messages: [
            { id: "e0", role: "user", content: "[checkpoint: window 1] …", at: 1000, handoff: { from: "ses_gen1", generation: 1, reason: "manual" } },
            { id: "e1", role: "user", content: "When do we ship?", at: 2000 },
            { id: "e2", role: "assistant", content: "Friday.", at: 3000, model: "glm-4.7-flash" },
          ],
        }
      },
      getLastSessionLog: async () => null,
      getResumeHandler: () => async (sessionId: string, log: any) => {
        calls.resumed.push([sessionId, log])
      },
    })
    const [[, log]] = calls.resumed as any
    expect(log.messages.map((m: any) => m.subtype ?? m.type)).toEqual([
      "informational", // 57 earlier messages not shown
      "compact_boundary", // that window's own rip (gen 2)
      "user",
      "assistant",
      "compact_boundary", // the head's rip (gen 3)
      "user", // checkpoint
    ])
    expect(log.messages[0].content).toContain("57 earlier messages")
    expect(log.messages[1].compactMetadata.handoff).toEqual({ generation: 2, reason: "manual" })
    expect(log.messages[2].timestamp).toBe(new Date(2000).toISOString())
    expect(log.messages[3].message.model).toBe("glm-4.7-flash")
    expect(log.messages[4].compactMetadata.handoff).toEqual({ generation: 3, reason: "threshold", earlierAbove: true })
    expect(log.messages.every((m: any) => m.sessionId === "ses_head_3")).toBe(true)
    // Only the checkpoint reaches the model.
    const context = getMessagesAfterCompactBoundary(log.messages)
    expect(context.map((m: any) => m.subtype ?? m.type)).toEqual(["compact_boundary", "user"])
  })

  test("a head the REPL already knows resumes its own transcript", async () => {
    const { createBot, getBot } = await import("../../src/runtime/bots/bot-store")
    await createBot({ name: "scout", title: "Scout" })
    const { openBotCanonicalChat } = await import(
      "../../src/cli/ui/ink-app/screens/bots-pane/open-bot-chat"
    )
    const headLog = { sessionId: "ses_head_2", messages: [{ uuid: "m1" }] }
    await openBotCanonicalChat("scout", {
      ...fakeDeps(),
      getBot,
      resolveHandoffHead: async () => ({ sessionId: "ses_head_2", checkpoint: "x" }),
      getLastSessionLog: async (id: string) => (id === "ses_head_2" ? headLog : null),
      getResumeHandler: () => async (sessionId: string, log: unknown) => {
        calls.resumed.push([sessionId, log])
      },
    })
    expect(calls.resumed).toEqual([["ses_head_2", headLog]])
  })

  test("opens the canonical chat, marks read, switches the ink session", async () => {
    const { createBot, getBot } = await import("../../src/runtime/bots/bot-store")
    await createBot({ name: "scout", title: "Scout" })

    const { openBotCanonicalChat } = await import(
      "../../src/cli/ui/ink-app/screens/bots-pane/open-bot-chat"
    )
    const result = await openBotCanonicalChat("scout", {
      ...fakeDeps(),
      getBot,
    })

    expect(calls.open).toBe(1)
    expect(calls.read).toEqual(["scout"])
    expect(calls.switched).toEqual([["ses_canonical_1", "/proj/demo"]])
    expect(result).toEqual({
      projectPath: "/proj/demo",
      sessionId: "ses_canonical_1",
      created: false,
    })
  })

  test("throws for an unknown bot without switching anything", async () => {
    const { getBot } = await import("../../src/runtime/bots/bot-store")
    const { openBotCanonicalChat } = await import(
      "../../src/cli/ui/ink-app/screens/bots-pane/open-bot-chat"
    )
    await expect(openBotCanonicalChat("ghost", { ...fakeDeps(), getBot })).rejects.toThrow(
      "not found",
    )
    expect(calls.open).toBe(0)
    expect(calls.read).toEqual([])
    expect(calls.switched).toEqual([])
  })

  test("uses the full resume pipeline when REPL publishes one", async () => {
    const { createBot, getBot } = await import("../../src/runtime/bots/bot-store")
    await createBot({ name: "scout", title: "Scout" })

    const { openBotCanonicalChat } = await import(
      "../../src/cli/ui/ink-app/screens/bots-pane/open-bot-chat"
    )
    await openBotCanonicalChat("scout", {
      ...fakeDeps(),
      getBot,
      getResumeHandler: () => async (sessionId: string, log: unknown, entrypoint: string) => {
        calls.resumed.push([sessionId, log, entrypoint])
      },
    })

    // the /resume-grade pipeline handles the transcript reload — the bare
    // switchSession fallback must NOT fire alongside it
    expect(calls.resumed).toEqual([
      ["ses_canonical_1", { sessionId: "ses_canonical_1", messages: [] }, "bots_pane"],
    ])
    expect(calls.switched).toEqual([])
  })

  test("freshly created canonical chat (no transcript) falls back to switchSession", async () => {
    const { createBot, getBot } = await import("../../src/runtime/bots/bot-store")
    await createBot({ name: "scout", title: "Scout" })

    const { openBotCanonicalChat } = await import(
      "../../src/cli/ui/ink-app/screens/bots-pane/open-bot-chat"
    )
    await openBotCanonicalChat("scout", {
      ...fakeDeps(),
      getBot,
      openCanonicalChat: async (bot: any) => {
        calls.open++
        return { projectPath: "/proj/demo", sessionId: "ses_fresh_1", created: true }
      },
      getResumeHandler: () => async (...args: unknown[]) => {
        calls.resumed.push(args)
      },
    })

    expect(calls.resumed).toEqual([])
    expect(calls.switched).toEqual([["ses_fresh_1", "/proj/demo"]])
  })
})
