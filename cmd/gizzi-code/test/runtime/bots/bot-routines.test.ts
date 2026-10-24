// @ts-nocheck
import { afterAll, beforeAll, describe, expect, mock, spyOn, test } from "bun:test"
import { rmSync } from "node:fs"
import { join } from "node:path"
import { tmpdir } from "../../fixture/fixture"
import { createBot, getBot, pinCanonicalSession, botDir } from "../../../src/runtime/bots/bot-store"

/**
 * Phase B3 — bot routines (D3): cron agent jobs with config.bot deliver into
 * the bot's canonical chat (never Session.createNext), are namespaced
 * `[bot:<name>] <label>`, and mark every delivery with `[routine: <label>]`.
 *
 * The `@/runtime/session` module is mocked process-wide BEFORE the executor
 * is imported: the bot delivery path must never touch it (no ephemeral
 * session minted, nothing deleted), while the mock's createNext/remove spies
 * would fire loudly if it did. No live model, no sqlite session store.
 */
const sessionCalls = { createNext: 0, remove: 0, get: 0, messages: 0 }

mock.module("../../src/runtime/session", () => ({
  Session: {
    createNext: async (opts) => {
      sessionCalls.createNext++
      return { id: `ses_created_${sessionCalls.createNext}`, directory: opts?.directory }
    },
    remove: async (id) => {
      sessionCalls.remove++
    },
    get: async (id) => {
      sessionCalls.get++
      throw new Error(`no such session: ${id}`)
    },
    messages: async () => {
      sessionCalls.messages++
      return []
    },
    list: () => [],
  },
}))

const { AgentExecutor } = await import(
  "../../../src/runtime/automation/cron/executors/agent-executor"
)
const {
  botRoutineJobName,
  botRoutineLabel,
  botRoutinePrefix,
  parseBotRoutineJobName,
  isRoutineTurnText,
  ROUTINE_MARKER_PREFIX,
  addBotRoutine,
  listBotRoutines,
  removeBotRoutine,
  cronServiceDeps,
  deliverBotRoutine,
} = await import("../../../src/runtime/bots/bot-routines")

let tmp: { path: string }

beforeAll(async () => {
  tmp = await tmpdir()
  process.env.GIZZI_CONFIG_DIR = join(tmp.path, ".gizzi")
})

afterAll(() => {
  delete process.env.GIZZI_CONFIG_DIR
  rmSync(join(tmp.path, ".gizzi"), { recursive: true, force: true })
})

/* In-memory delivery seam — records everything, touches nothing. */
function fakeDelivery(overrides: Record<string, any> = {}) {
  const calls: {
    openedWith?: { bot: string; projectPath: string }
    turn?: any
  } = {}
  const deps = {
    async getBot(name: string) {
      return getBot(name)
    },
    async openCanonicalSession(bot: any, projectPath: string) {
      calls.openedWith = { bot: bot.name, projectPath }
      return { projectPath, sessionId: "ses_canonical_pin", created: false }
    },
    async runTurn(input: any) {
      calls.turn = input
    },
    async lastAssistant(sessionID: string) {
      return { text: "routine done", tokensUsed: 42 }
    },
    ...overrides,
  }
  return { deps, calls }
}

function fakeRun() {
  return {
    id: "run-1",
    jobId: "job-1",
    status: "running",
    attempt: 1,
    triggeredBy: "schedule",
    metadata: {},
  }
}

function botJob(name: string, config: Record<string, any>) {
  return { id: "job-1", name, type: "agent", config }
}

describe("[bot:<name>] namespace", () => {
  test("jobName/prefix/parse round trip", () => {
    expect(botRoutinePrefix("alice")).toBe("[bot:alice]")
    expect(botRoutineJobName("alice", "morning check")).toBe("[bot:alice] morning check")
    expect(parseBotRoutineJobName("[bot:alice] morning check")).toEqual({
      bot: "alice",
      label: "morning check",
    })
  })

  test("parse returns null for non-routine jobs", () => {
    expect(parseBotRoutineJobName("nightly backup")).toBeNull()
    expect(parseBotRoutineJobName("[bot:] empty")).toBeNull()
    // a longer bot name must not parse as a shorter one's job
    expect(parseBotRoutineJobName("[bot:alice2] sweep")!.bot).toBe("alice2")
  })

  test("botRoutineLabel strips the namespace, falls back to the full name", () => {
    expect(botRoutineLabel("[bot:alice] morning check")).toBe("morning check")
    expect(botRoutineLabel("plain job")).toBe("plain job")
  })

  test("isRoutineTurnText detects the delivery marker", () => {
    expect(isRoutineTurnText(`${ROUTINE_MARKER_PREFIX}morning check] sweep the inbox`)).toBe(true)
    expect(isRoutineTurnText("[routine: ] edge")).toBe(true)
    expect(isRoutineTurnText("hello there")).toBe(false)
    expect(isRoutineTurnText(" [routine: indented] not a marker")).toBe(false)
  })
})

describe("executor dispatch (D3)", () => {
  test("bot job resolves the canonical session and never calls Session.createNext", async () => {
    await createBot({ name: "rbot", title: "Routine Bot" })
    await pinCanonicalSession("rbot", { projectPath: "/pinned/proj", sessionId: "ses_canonical_pin" })
    const { deps, calls } = fakeDelivery()
    const executor = new AgentExecutor({ defaultCwd: "/cwd", botRoutineDeps: deps })
    const run = fakeRun()

    await executor.execute(
      botJob("[bot:rbot] morning check", { prompt: "check the inbox", bot: "rbot" }),
      run,
      new AbortController().signal,
    )

    // resumed the pinned session (not cwd) — one marked turn
    expect(calls.openedWith).toEqual({ bot: "rbot", projectPath: "/pinned/proj" })
    expect(calls.turn.sessionID).toBe("ses_canonical_pin")
    expect(calls.turn.parts[0].text).toBe("[routine: morning check] check the inbox")
    // run record captures the turn result
    expect(run.response).toBe("routine done")
    expect(run.output).toBe("routine done")
    expect(run.tokensUsed).toBe(42)
    expect(run.agentId).toBe("bot:rbot")
    // the ephemeral-session machinery stayed untouched
    expect(sessionCalls.createNext).toBe(0)
    expect(sessionCalls.remove).toBe(0)
  })

  test("a Stop cancels the bot's turn in its canonical session", async () => {
    await createBot({ name: "stopbot", title: "Stop Bot" })
    const { SessionPrompt } = await import("../../../src/runtime/session/prompt")
    const cancel = spyOn(SessionPrompt, "cancel").mockImplementation(() => {})
    const controller = new AbortController()
    let finishTurn = () => {}
    const { deps } = fakeDelivery({
      async runTurn() {
        // The turn is in flight when the user presses Stop.
        const done = new Promise<void>((resolve) => (finishTurn = resolve))
        controller.abort(new Error("Stopped by user"))
        await done
      },
    })
    const original = cancel.getMockImplementation()
    cancel.mockImplementation((id: string) => {
      original?.(id)
      finishTurn()
    })
    const run = fakeRun()
    await new AgentExecutor({ defaultCwd: "/cwd", botRoutineDeps: deps }).execute(
      botJob("[bot:stopbot] long job", { prompt: "go", bot: "stopbot" }),
      run,
      controller.signal,
    )
    expect(cancel).toHaveBeenCalledWith("ses_canonical_pin")
    // The partial answer is still the run's output.
    expect(run.output).toBe("routine done")
    cancel.mockRestore()
  })

  test("never-pinned bot opens the canonical chat in the executor cwd", async () => {
    await createBot({ name: "unpinned", title: "Unpinned" })
    const { deps, calls } = fakeDelivery()
    const executor = new AgentExecutor({ defaultCwd: "/fallback/cwd", botRoutineDeps: deps })

    await executor.execute(
      botJob("[bot:unpinned] sweep", { prompt: "tidy up", bot: "unpinned" }),
      fakeRun(),
      new AbortController().signal,
    )

    expect(calls.openedWith).toEqual({ bot: "unpinned", projectPath: "/fallback/cwd" })
    expect(sessionCalls.createNext).toBe(0)
  })

  test("context is folded into the marked prompt before the marker", async () => {
    await createBot({ name: "ctxbot", title: "Ctx" })
    const { deps, calls } = fakeDelivery()
    const executor = new AgentExecutor({ defaultCwd: "/cwd", botRoutineDeps: deps })

    await executor.execute(
      botJob("[bot:ctxbot] digest", {
        prompt: "write the digest",
        context: "background info",
        bot: "ctxbot",
      }),
      fakeRun(),
      new AbortController().signal,
    )

    expect(calls.turn.parts[0].text).toBe("[routine: digest] background info\n\nwrite the digest")
    expect(sessionCalls.createNext).toBe(0)
  })

  test("model and agent pass through to the turn", async () => {
    await createBot({ name: "modelbot", title: "Model" })
    const { deps, calls } = fakeDelivery()
    const executor = new AgentExecutor({ defaultCwd: "/cwd", botRoutineDeps: deps })

    await executor.execute(
      botJob("[bot:modelbot] m", {
        prompt: "p",
        bot: "modelbot",
        model: "anthropic/claude-sonnet-4-5",
        agentId: "researcher",
      }),
      fakeRun(),
      new AbortController().signal,
    )

    expect(calls.turn.model).toEqual({ providerID: "anthropic", modelID: "claude-sonnet-4-5" })
    expect(calls.turn.agent).toBe("researcher")
  })

  test("unknown bot records a failed run (structured error, no session touched)", async () => {
    const { deps } = fakeDelivery()
    const executor = new AgentExecutor({ defaultCwd: "/cwd", botRoutineDeps: deps })
    const run = fakeRun()

    await expect(
      executor.execute(
        botJob("[bot:ghost] haunt", { prompt: "boo", bot: "ghost" }),
        run,
        new AbortController().signal,
      ),
    ).rejects.toThrow(/bot 'ghost' not found/)

    expect(sessionCalls.createNext).toBe(0)
    expect(sessionCalls.remove).toBe(0)
  })

  test("deliverBotRoutine without injected deps uses the bot store for lookup", async () => {
    await createBot({ name: "direct", title: "Direct" })
    // getBot override comes from the real store; session open is faked via deps
    const { deps, calls } = fakeDelivery()
    const delivery = await deliverBotRoutine({
      botName: "direct",
      label: "l",
      prompt: "p",
      projectPath: "/cwd",
      deps,
    })
    expect(delivery.sessionId).toBe("ses_canonical_pin")
    expect(delivery.createdSession).toBe(false)
    expect(calls.turn.parts[0].text).toBe("[routine: l] p")
  })
})

/* In-memory cron store seam for CRUD tests. */
function fakeCronDeps() {
  const jobs = new Map<string, any>()
  let counter = 0
  const deps = {
    create(input: any) {
      const job = { ...input, id: `job_fake_${++counter}`, status: "active" }
      jobs.set(job.id, job)
      return job
    },
    list() {
      return [...jobs.values()]
    },
    get(id: string) {
      return jobs.get(id) ?? null
    },
    remove(id: string) {
      return jobs.delete(id)
    },
  }
  return { deps, jobs }
}

describe("routine CRUD over the cron store", () => {
  test("addBotRoutine namespaces the job, sets config.bot + catchUpMissed", async () => {
    await createBot({ name: "crud-a", title: "A" })
    const { deps, jobs } = fakeCronDeps()

    const job = await addBotRoutine(
      "crud-a",
      { schedule: "daily at 9am", prompt: "say hi" },
      deps,
    )

    expect(job.name).toBe("[bot:crud-a] say hi")
    expect(job.type).toBe("agent")
    expect(job.config).toEqual({ prompt: "say hi", bot: "crud-a" })
    expect(job.catchUpMissed).toBe(true)
    expect(jobs.has(job.id)).toBe(true)

    const labeled = await addBotRoutine(
      "crud-a",
      { label: "morning check", schedule: "0 9 * * *", prompt: "check things" },
      deps,
    )
    expect(labeled.name).toBe("[bot:crud-a] morning check")
  })

  test("addBotRoutine rejects unknown bots and bad schedules", async () => {
    const { deps } = fakeCronDeps()
    await expect(
      addBotRoutine("no-such-bot", { schedule: "daily", prompt: "x" }, deps),
    ).rejects.toThrow(/bot 'no-such-bot' not found/)
  })

  test("listBotRoutines returns only the bot's own jobs", async () => {
    await createBot({ name: "list-a", title: "A" })
    await createBot({ name: "list-b", title: "B" })
    const { deps } = fakeCronDeps()
    await addBotRoutine("list-a", { label: "a1", schedule: "daily", prompt: "p1" }, deps)
    await addBotRoutine("list-a", { label: "a2", schedule: "daily", prompt: "p2" }, deps)
    await addBotRoutine("list-b", { label: "b1", schedule: "daily", prompt: "p3" }, deps)
    deps.create({ name: "not a bot job", type: "shell", schedule: "daily", config: {} })

    const mine = await listBotRoutines("list-a", deps)
    expect(mine.map((j) => botRoutineLabel(j.name)).sort()).toEqual(["a1", "a2"])

    expect(await listBotRoutines("list-b", deps)).toHaveLength(1)
    await expect(listBotRoutines("ghost", deps)).rejects.toThrow(/not found/)
  })

  test("removeBotRoutine is prefix-scoped to the owning bot", async () => {
    await createBot({ name: "own-a", title: "A" })
    await createBot({ name: "own-b", title: "B" })
    const { deps } = fakeCronDeps()
    const aJob = await addBotRoutine("own-a", { label: "sweep", schedule: "daily", prompt: "p" }, deps)

    // another bot cannot remove it
    await expect(removeBotRoutine("own-b", aJob.id, deps)).rejects.toThrow(
      /does not belong to bot 'own-b'/,
    )
    // non-bot jobs are refused too
    const plain = deps.create({ name: "plain", type: "shell", schedule: "daily", config: {} })
    await expect(removeBotRoutine("own-a", plain.id, deps)).rejects.toThrow(
      /does not belong to bot 'own-a'/,
    )
    // unknown job
    await expect(removeBotRoutine("own-a", "job_missing", deps)).rejects.toThrow(
      /cron job 'job_missing' not found/,
    )
    // owner removes it
    const removed = await removeBotRoutine("own-a", aJob.id, deps)
    expect(removed.name).toBe("[bot:own-a] sweep")
    expect((await listBotRoutines("own-a", deps))).toHaveLength(0)
  })
})

describe("cronServiceDeps (real CronService over a temp db)", () => {  test("add/list/remove round trip against SQLite", async () => {
    await createBot({ name: "db-bot", title: "DB Bot" })
    const dbPath = join(tmp.path, "cron-test.db")
    const deps = await cronServiceDeps(dbPath)

    const job = await addBotRoutine(
      "db-bot",
      { label: "db sweep", schedule: "0 9 * * *", prompt: "sweep" },
      deps,
    )
    expect(deps.get(job.id)?.name).toBe("[bot:db-bot] db sweep")
    expect((await listBotRoutines("db-bot", deps)).map((j) => j.id)).toContain(job.id)

    await removeBotRoutine("db-bot", job.id, deps)
    expect(deps.get(job.id)).toBeNull()

    const { CronService } = await import("../../../src/runtime/automation/cron/service")
    CronService.close()
  })
})

describe("typed retry on bot wake (B4/D4)", () => {
  test("transient failure retries the same delivery once, then succeeds", async () => {
    await createBot({ name: "retry-once", title: "Retry Once" })
    let attempts = 0
    const { deps, calls } = fakeDelivery({
      async runTurn(input: any) {
        attempts++
        calls.turn = input
        if (attempts === 1) throw new Error("HTTP 429 too many requests")
      },
    })
    const compact = { called: 0 }
    deps.compactSession = async () => {
      compact.called++
    }

    const delivery = await deliverBotRoutine({
      botName: "retry-once",
      label: "l",
      prompt: "p",
      projectPath: "/cwd",
      deps,
    })

    expect(attempts).toBe(2)
    expect(compact.called).toBe(0)
    expect(delivery.response).toBe("routine done")
  })

  test("context_overflow compacts in place, then retries once", async () => {
    await createBot({ name: "retry-compact", title: "Retry Compact" })
    let attempts = 0
    const compacted: string[] = []
    const { deps } = fakeDelivery({
      async runTurn() {
        attempts++
        if (attempts === 1) throw new Error("maximum context length exceeded")
      },
    })
    deps.compactSession = async (sessionID: string) => {
      compacted.push(sessionID)
    }

    const delivery = await deliverBotRoutine({
      botName: "retry-compact",
      label: "l",
      prompt: "p",
      projectPath: "/cwd",
      deps,
    })

    expect(attempts).toBe(2)
    // compaction ran against the canonical session the delivery opened
    expect(compacted).toEqual(["ses_canonical_pin"])
    expect(delivery.sessionId).toBe("ses_canonical_pin")
  })

  test("auth/quota/config failures never auto-retry and carry the typed reason", async () => {
    await createBot({ name: "retry-never", title: "Retry Never" })
    let attempts = 0
    const { deps } = fakeDelivery({
      async runTurn() {
        attempts++
        throw Object.assign(new Error("HTTP 401 invalid api key"), { statusCode: 401 })
      },
    })

    const err = await deliverBotRoutine({
      botName: "retry-never",
      label: "l",
      prompt: "p",
      projectPath: "/cwd",
      deps,
    }).then(
      () => null,
      (e) => e,
    )

    expect(attempts).toBe(1)
    expect(err).not.toBeNull()
    expect(err.reason).toBe("provider_auth_or_access")
  })

  test("exhausted once-retry rethrows with the typed reason of the final error", async () => {
    await createBot({ name: "retry-exhausted", title: "Retry Exhausted" })
    let attempts = 0
    const { deps } = fakeDelivery({
      async runTurn() {
        attempts++
        throw new Error("HTTP 503 service unavailable")
      },
    })

    const err = await deliverBotRoutine({
      botName: "retry-exhausted",
      label: "l",
      prompt: "p",
      projectPath: "/cwd",
      deps,
    }).then(
      () => null,
      (e) => e,
    )

    expect(attempts).toBe(2)
    expect(err.reason).toBe("provider_server_error")
  })

  test("executor records the typed reason on the failed run record", async () => {
    await createBot({ name: "run-reason", title: "Run Reason" })
    const { deps } = fakeDelivery({
      async runTurn() {
        throw new Error("no llm provider configured")
      },
    })
    const executor = new AgentExecutor({ defaultCwd: "/cwd", botRoutineDeps: deps })
    const run = fakeRun()

    await expect(
      executor.execute(
        botJob("[bot:run-reason] wake", { prompt: "p", bot: "run-reason" }),
        run,
        new AbortController().signal,
      ),
    ).rejects.toThrow(/no llm provider configured/)

    expect(run.metadata.reason).toBe("missing_config")
    expect(run.reason).toBe("missing_config")
    // delivery machinery never minted or removed an ephemeral session
    expect(sessionCalls.createNext).toBe(0)
    expect(sessionCalls.remove).toBe(0)
  })

  test("typed reason persists as a real column through CronDatabase (fresh + migrated db)", async () => {
    const { CronDatabase } = await import("../../../src/runtime/automation/cron/database")
    await using tmp = await tmpdir()
    const dbPath = join(tmp.path, "cron.db")

    // fresh db: reason column from CREATE TABLE
    const db = new CronDatabase(dbPath)
    db.saveRun({
      ...fakeRun(),
      status: "failed",
      scheduledAt: new Date().toISOString(),
      finishedAt: new Date().toISOString(),
      error: "no llm provider configured",
      reason: "missing_config",
      metadata: { reason: "missing_config" },
    })
    expect(db.getRun("run-1")?.reason).toBe("missing_config")
    db.close()

    // pre-existing db without the column: open migrates it, data survives
    const { Database } = await import("bun:sqlite")
    const raw = new Database(dbPath)
    raw.exec("CREATE TABLE runs_backup AS SELECT * FROM runs")
    raw.exec("DROP TABLE runs")
    raw.exec(`CREATE TABLE runs (
      id TEXT PRIMARY KEY, job_id TEXT NOT NULL, status TEXT NOT NULL,
      scheduled_at TEXT NOT NULL, started_at TEXT, finished_at TEXT,
      duration_ms INTEGER, attempt INTEGER NOT NULL DEFAULT 1, retry_of TEXT,
      output TEXT, error TEXT, exit_code INTEGER, http_status INTEGER,
      agent_id TEXT, response TEXT, tokens_used INTEGER,
      triggered_by TEXT NOT NULL, triggered_by_user TEXT, metadata TEXT)`)
    raw.exec(`INSERT INTO runs SELECT id, job_id, status, scheduled_at, started_at,
      finished_at, duration_ms, attempt, retry_of, output, error, exit_code,
      http_status, agent_id, response, tokens_used, triggered_by,
      triggered_by_user, metadata FROM runs_backup`)
    raw.exec("DROP TABLE runs_backup")
    raw.close()

    const migrated = new CronDatabase(dbPath)
    // old rows survive without a reason (they never stored one); the column
    // now exists and accepts new typed reasons
    expect(migrated.getRun("run-1")?.error).toBe("no llm provider configured")
    expect(migrated.getRun("run-1")?.reason).toBeNull()
    migrated.saveRun({
      ...fakeRun(),
      id: "run-2",
      status: "failed",
      scheduledAt: new Date().toISOString(),
      finishedAt: new Date().toISOString(),
      error: "rate limited",
      reason: "provider_rate_limit",
      metadata: {},
    })
    expect(migrated.getRun("run-2")?.reason).toBe("provider_rate_limit")
    migrated.close()
  })
})
