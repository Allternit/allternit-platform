import { describe, expect, test } from "bun:test"
import { IDLE_AFTER_SEND_MS, observeWorker, parseBrain, type SwarmRecord, type SwarmWorker } from "../../src/cli/factory/swarm"
import { badgeText, buildBoard, allRows } from "../../src/cli/factory/board"
import { lastMeaningfulLine } from "../../src/cli/factory/live"
import type { FactoryAgent } from "../../src/cli/factory/types"

const T0 = 1_000_000

function worker(over: Partial<SwarmWorker> = {}): SwarmWorker {
  return {
    bot: "w1",
    address: "w1@sw-x",
    agentId: "local:w1-sw-x",
    slug: "w1-sw-x",
    branch: "swarm/sw-x/w1",
    workdir: "/tmp/r-sw-x-w1",
    startedAt: T0,
    phase: "starting",
    ...over,
  }
}

const agent = (state: FactoryAgent["state"]): FactoryAgent => ({ id: "local:w1-sw-x", address: "w1@sw-x", state, pane: { id: "w1:p2", attachable: true } })

describe("swarm worker phases", () => {
  test("idle before its task is sent is not done", () => {
    expect(observeWorker(worker(), agent("idle"), T0 + 60_000).phase).toBe("starting")
  })

  test("working then idle is done, and done stays done while the engine says idle", () => {
    let w = worker({ taskSentAt: T0 + 1000 })
    w = observeWorker(w, agent("working"), T0 + 2000)
    expect(w.phase).toBe("working")
    w = observeWorker(w, agent("idle"), T0 + 3000)
    expect(w.phase).toBe("done")
    expect(observeWorker(w, agent("idle"), T0 + 9_000_000).phase).toBe("done")
  })

  test("a queued task (mailbox) is not done until the worker has worked on it", () => {
    const w = worker({ taskSentAt: T0, taskQueued: true })
    expect(observeWorker(w, agent("idle"), T0 + IDLE_AFTER_SEND_MS * 10).phase).toBe("starting")
  })

  test("a verified task that never shows work is done after the idle window", () => {
    const w = worker({ taskSentAt: T0 })
    expect(observeWorker(w, agent("idle"), T0 + IDLE_AFTER_SEND_MS - 1).phase).toBe("starting")
    expect(observeWorker(w, agent("idle"), T0 + IDLE_AFTER_SEND_MS + 1).phase).toBe("done")
  })

  test("a steered done worker goes back to working, and needs_you is waiting", () => {
    const done = worker({ taskSentAt: T0, phase: "done", finishedAt: T0 + 5, sawWorking: true })
    expect(observeWorker(done, agent("working"), T0 + 10).phase).toBe("working")
    expect(observeWorker(done, agent("needs_you"), T0 + 10).phase).toBe("waiting")
  })

  test("a pane that disappears after its task was sent is failed", () => {
    const w = observeWorker(worker({ taskSentAt: T0, phase: "working", sawWorking: true }), undefined, T0 + 10)
    expect(w.phase).toBe("failed")
  })
})

describe("brains", () => {
  test("CLI brains map to engine harnesses", () => {
    expect(parseBrain("claude-cli/claude-sonnet-5")).toEqual({ harness: "claude", model: "claude-sonnet-5" })
    expect(parseBrain("codex-cli")).toEqual({ harness: "codex", model: null })
    expect(() => parseBrain("openai/gpt-5")).toThrow(/isn't a brain/)
  })
})

describe("board rows", () => {
  const rec: SwarmRecord = {
    id: "sw-x",
    task: "t",
    brain: "claude-cli/claude-sonnet-5",
    harness: "claude",
    model: "claude-sonnet-5",
    root: "/tmp/r",
    baseBranch: "main",
    createdAt: T0,
    workers: [worker({ phase: "done", finishedAt: T0 + 60_000 }), worker({ bot: "w2", address: "w2@sw-x", agentId: "local:w2-sw-x", phase: "landed" })],
  }

  test("a finished worker stays on the board as done with a frozen age; landed ones leave; offline bots are hidden", () => {
    const others: FactoryAgent[] = [
      { id: "local:lead-t", address: "lead@t", state: "needs_you", binding: { type: "terminal", harness: "claude" } },
      { id: "local:old", address: "old@t", state: "offline" },
    ]
    const rows = allRows(buildBoard({ agents: [agent("idle"), ...others], swarms: [rec], lastLine: {}, since: {}, now: T0 + 999_999 }))
    expect(rows.map((r) => [r.address, r.state])).toEqual([
      ["w1@sw-x", "done"],
      ["lead@t", "waiting"],
    ])
    expect(rows[0]!.elapsedMs).toBe(60_000)
    expect(rows[0]!.cost).toBe("plan")
    expect(badgeText(rows)).toBe("⚙ 2 bots · 1 waiting")
  })

  test("no badge when nothing is working or waiting", () => {
    const rows = allRows(buildBoard({ agents: [agent("idle")], swarms: [rec], lastLine: {}, since: {}, now: T0 }))
    expect(badgeText(rows)).toBeNull()
  })
})

describe("last line of a pane", () => {
  test("skips the Claude Code prompt box and footer", () => {
    const screen = [
      "❯ Reply with PONG",
      "⏺ PONG",
      "✻ Cooked for 2s · done 11:08 AM",
      "─".repeat(40),
      "❯ ",
      "─".repeat(40),
      "  \x1b[2m⏵⏵ bypass permissions on (shift+tab to cycle)\x1b[0m",
    ].join("\n")
    expect(lastMeaningfulLine(screen)).toBe("✻ Cooked for 2s · done 11:08 AM")
  })
})
