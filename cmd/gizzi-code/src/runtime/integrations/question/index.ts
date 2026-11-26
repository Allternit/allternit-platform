import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Log } from "@/shared/util/log"
import { Database, and, eq, gte, inArray, lt } from "@/runtime/session/storage/db"
import { SessionTable } from "@/runtime/session/session.sql"
import { Scheduler } from "@/runtime/automation/scheduler"
import { QuestionAsyncTable } from "@/runtime/integrations/question/question.sql"
import z from "zod/v4"

export namespace Question {
  const log = Log.create({ service: "question" })

  const Leaf = z.object({
    label: z.string().describe("Display text (1-5 words, concise)"),
    description: z.string().describe("Explanation of choice, or what happens if picked"),
    recommended: z.boolean().optional().describe("Your pick (at most one per list)"),
    caution: z.string().optional().describe("A short cost or risk of this choice, shown highlighted (e.g. 'Skips 1,284 tests')"),
  })

  export const FollowUp = z.object({
    question: z.string().describe("What to settle next if this option is picked"),
    options: z.array(Leaf).describe("Choices for the follow-up"),
  })
  export type FollowUp = z.infer<typeof FollowUp>

  export const Option = Leaf.extend({
    then: FollowUp.optional().describe("Decision tree: a follow-up question shown when this option is picked"),
  })

  export type Option = z.infer<typeof Option>

  /** One step of a plan shown for approval (`plan_exit`). */
  export const PlanStep = z.object({
    title: z.string().describe("The step, one short line"),
    detail: z.string().optional().describe("Optional extra line under the step"),
  })
  export type PlanStep = z.infer<typeof PlanStep>

  export const Info = z
    .object({
      question: z.string().describe("Complete question"),
      header: z.string().describe("Very short label (max 30 chars)"),
      options: z.array(Option).describe("Available choices"),
      multiple: z.boolean().optional().describe("Allow selecting multiple choices"),
      custom: z.boolean().optional().describe("Allow typing a custom answer (default: true)"),
      risk: z.enum(["low", "medium", "high"]).optional().describe("Decision tree: how much is at stake"),
      context: z.string().optional().describe("Decision tree: the facts behind the question, one or two short lines"),
      plan: z
        .object({
          steps: z.array(PlanStep),
          page: z.string().optional().describe("Link to the full plan as a page (set by the answer-me plan bridge)"),
          body: z.string().optional().describe("The plan as Markdown (capped at MAX_PLAN_BODY)"),
          title: z.string().optional().describe("The plan's title"),
          artifact: z
            .object({
              id: z.string(),
              version: z.number(),
              persisted: z.boolean().describe("false = not saved to the account (offline or old server); `body` still carries the plan"),
            })
            .optional()
            .describe("The plan artifact (art_plan_<session id>) this plan was saved as"),
        })
        .optional()
        .describe("Plan approval: steps the person can edit, add, remove and reorder before answering"),
    })

  export type Info = z.infer<typeof Info>

  export const MAX_PLAN_STEPS = 50
  export const MAX_PLAN_TEXT = 500
  export const MAX_PLAN_BODY = 100_000

  /** An edited plan from a reply, kept only when it is a valid, bounded step list. */
  export function readPlan(value: unknown): PlanStep[] | undefined {
    if (!Array.isArray(value) || value.length > MAX_PLAN_STEPS) return undefined
    const steps: PlanStep[] = []
    for (const item of value) {
      if (!item || typeof item !== "object") return undefined
      const title = typeof (item as any).title === "string" ? (item as any).title.trim().slice(0, MAX_PLAN_TEXT) : ""
      if (!title) continue
      const detail = typeof (item as any).detail === "string" ? (item as any).detail.trim().slice(0, MAX_PLAN_TEXT) : ""
      steps.push(detail ? { title, detail } : { title })
    }
    return steps
  }

  /** A plan page link: an http(s) URL of bounded length, or undefined. */
  export function planPageUrl(value: unknown): string | undefined {
    if (typeof value !== "string" || value.length > 2000) return undefined
    try {
      const url = new URL(value)
      return url.protocol === "https:" || url.protocol === "http:" ? url.toString() : undefined
    } catch {
      return undefined
    }
  }

  /** A decision tree: any option has a follow-up, or the question carries risk/context. */
  export function isTree(q: Info) {
    return !q.multiple && (q.options.some((o) => o.then) || !!q.risk || !!q.context)
  }

  /** One answer as text: a tree answer is a path ("Roll back → Post on the status page"). */
  export function formatAnswer(q: Info, answer: Answer | undefined) {
    if (!answer?.length) return ""
    return answer.join(isTree(q) ? " → " : ", ")
  }

  export const Request = z
    .object({
      id: Identifier.schema("question"),
      sessionID: Identifier.schema("session"),
      questions: z.array(Info).describe("Questions to ask"),
      tool: z
        .object({
          messageID: z.string(),
          callID: z.string(),
        })
        .optional(),
      // ask_user_later: the turn did not wait; the answer starts a new turn.
      async: z.boolean().optional().describe("Asked with ask_user_later: answering starts a new turn"),
      assumption: z.string().optional().describe("What the agent is doing until the answer arrives"),
      time: z.object({ created: z.number() }).optional(),
    })

  export type Request = z.infer<typeof Request>

  export const Answer = z.array(z.string())
  export type Answer = z.infer<typeof Answer>

  export const Reply = z.object({
    answers: z
      .array(Answer)
      .describe("User answers in order of questions (each answer is an array of selected labels)"),
    plan: z.array(PlanStep).optional().describe("The plan as the person left it, for a question that carries `plan`"),
  })
  export type Reply = z.infer<typeof Reply>

  export const Event = {
    Asked: BusEvent.define("question.asked", Request),
    Replied: BusEvent.define(
      "question.replied",
      z.object({
        sessionID: z.string(),
        requestID: z.string(),
        answers: z.array(Answer),
      }),
    ),
    Rejected: BusEvent.define(
      "question.rejected",
      z.object({
        sessionID: z.string(),
        requestID: z.string(),
      }),
    ),
  }

  const state = Instance.state(async () => {
    const pending: Record<
      string,
      {
        info: Request
        resolve: (answers: Answer[], plan?: PlanStep[]) => void
        reject: (e: any) => void
      }
    > = {}

    // ask_user_later expiry: once when the instance starts, then hourly.
    Scheduler.register({ id: "question.async.expiry", interval: ASYNC_SWEEP_MS, run: async () => void (await expireDue()) })

    return {
      pending,
    }
  })

  // A question waiting for its answer keeps a settings reload from wiping it (Instance.disposeAll).
  Instance.registerBusyCheck(async () => Object.keys((await state()).pending).length > 0)

  /** Starts this instance's question state at boot, so the ask_user_later expiry sweep runs without waiting for the first question. */
  export async function init() {
    await state()
  }

  export async function ask(input: {
    sessionID: string
    questions: Info[]
    tool?: { messageID: string; callID: string }
  }): Promise<Answer[]> {
    return (await askWithPlan(input)).answers
  }

  /** Like `ask`, and also returns the plan as the person edited it (plan questions). */
  export async function askWithPlan(input: {
    sessionID: string
    questions: Info[]
    tool?: { messageID: string; callID: string }
  }): Promise<{ answers: Answer[]; plan?: PlanStep[] }> {
    const s = await state()
    const id = Identifier.ascending("question")
    log.info("asking", { id, questions: input.questions.length })
    return new Promise((resolve, reject) => {
      const info: Request = { id, sessionID: input.sessionID, questions: input.questions, tool: input.tool }
      s.pending[id] = { info, resolve: (answers, plan) => resolve({ answers, plan }), reject }
      Bus.publish(Event.Asked, info)
    })
  }

  /**
   * A question asked in another process (a live terminal session, see
   * TerminalLive). Published like any other, so every surface shows it and
   * answers it through `reply`/`reject`; settles with the answers, or
   * undefined when it was declined.
   */
  export async function relay(info: Request): Promise<Answer[] | undefined> {
    const s = await state()
    return new Promise((resolve) => {
      s.pending[info.id] = { info, resolve: (answers) => resolve(answers), reject: () => resolve(undefined) }
      Bus.publish(Event.Asked, info)
    })
  }

  /** A relayed question answered (or dropped) where it was asked: close it everywhere. */
  export async function settle(requestID: string, answers: Answer[] | undefined) {
    const s = await state()
    const existing = s.pending[requestID]
    if (!existing) return
    delete s.pending[requestID]
    const sessionID = existing.info.sessionID
    if (answers) Bus.publish(Event.Replied, { sessionID, requestID, answers })
    else Bus.publish(Event.Rejected, { sessionID, requestID })
  }

  export async function reply(input: { requestID: string; answers: Answer[]; plan?: unknown }): Promise<void> {
    const s = await state()
    const existing = s.pending[input.requestID]
    if (!existing) {
      if (await replyAsync(input.requestID, input.answers)) return
      log.warn("reply for unknown request", { requestID: input.requestID })
      return
    }
    delete s.pending[input.requestID]

    log.info("replied", { requestID: input.requestID, answers: input.answers })

    Bus.publish(Event.Replied, {
      sessionID: existing.info.sessionID,
      requestID: existing.info.id,
      answers: input.answers,
    })

    existing.resolve(input.answers, readPlan(input.plan))
  }

  /**
   * Links a pending plan question to the same plan shown as a page (the
   * answer-me plan bridge). Apps show it as "Open full plan". Returns false for
   * an unknown request, a question without a plan, or a bad URL.
   */
  export async function setPlanPage(requestID: string, value: unknown): Promise<boolean> {
    const url = planPageUrl(value)
    const existing = (await state()).pending[requestID]
    if (!url || !existing) return false
    let set = false
    for (const q of existing.info.questions) {
      if (!q.plan) continue
      q.plan.page = url
      set = true
    }
    return set
  }

  export async function reject(requestID: string): Promise<void> {
    const s = await state()
    const existing = s.pending[requestID]
    if (!existing) {
      if (await closeAsync(requestID, "dismissed")) return
      log.warn("reject for unknown request", { requestID })
      return
    }
    delete s.pending[requestID]

    log.info("rejected", { requestID })

    Bus.publish(Event.Rejected, {
      sessionID: existing.info.sessionID,
      requestID: existing.info.id,
    })

    existing.reject(new RejectedError())
  }

  export class RejectedError extends Error {
    constructor() {
      super("The user dismissed this question")
    }
  }

  /** Questions waiting for an answer: blocking ones in memory, then ask_user_later asks from the database. */
  export async function list(): Promise<Request[]> {
    const live = await state().then((x) => Object.values(x.pending).map((x) => x.info))
    return [...live, ...listAsync()]
  }

  /*
   * ask_user_later: questions whose turn does not wait. Each ask is a row in
   * question_async (persisted, so it survives a restart). Answering starts a
   * new turn in the thread, like a routine delivery; an ask nobody answers
   * expires after ASYNC_EXPIRY_MS, silently to the model (the card shows
   * "Expired"). A subagent's ask belongs to its root session.
   */

  export const ASYNC_MAX_PENDING = 3
  export const ASYNC_MAX_QUESTIONS = 3
  export const ASYNC_MAX_ASSUMPTION = 300
  export const ASYNC_EXPIRY_MS = 7 * 24 * 60 * 60 * 1000
  const ASYNC_SWEEP_MS = 60 * 60 * 1000
  const MAX_PARENT_DEPTH = 32

  /** Marks the text part of a turn started by an answer (`[answer to your question <id>]`). */
  export const ANSWER_MARK = "answer to your question"

  export class AsyncLimitError extends Error {
    constructor(public readonly pending: number) {
      super(
        `This thread already has ${pending} questions waiting for the user (the limit is ${ASYNC_MAX_PENDING}). ` +
          "Don't ask another one now: continue with your assumptions, and ask again after one is answered or expires.",
      )
    }
  }

  /** How an answer becomes a turn. A seam so tests can watch it without a model. */
  export const asyncTurn = {
    async start(input: { sessionID: string; requestID: string; text: string }): Promise<void> {
      const { SessionPrompt } = await import("@/runtime/session/prompt")
      // Same path as a routine delivery: a user turn; a busy session queues it.
      await SessionPrompt.prompt({
        sessionID: input.sessionID,
        parts: [{ type: "text", text: input.text, metadata: { askUserLater: { id: input.requestID } } }],
      })
    },
  }

  /** The thread a session belongs to: walk parentID up to the top session. */
  export async function rootSessionID(sessionID: string): Promise<string> {
    const { Session } = await import("@/runtime/session")
    let id = sessionID
    const seen = new Set<string>()
    for (let depth = 0; depth < MAX_PARENT_DEPTH && !seen.has(id); depth++) {
      seen.add(id)
      const info = await Session.get(id).catch(() => undefined)
      if (!info?.parentID) break
      id = info.parentID
    }
    return id
  }

  type AsyncRow = typeof QuestionAsyncTable.$inferSelect

  function asyncRequest(row: AsyncRow): Request {
    return {
      id: row.id,
      sessionID: row.root_session_id,
      questions: row.questions as Info[],
      async: true,
      assumption: row.assumption,
      time: { created: row.created_at },
    }
  }

  function expiryCutoff(now = Date.now()) {
    return now - ASYNC_EXPIRY_MS
  }

  /**
   * Every session this runtime stores, in any project. A live terminal session
   * sits in the default project while Desktop or a phone may ask through
   * another project's instance, so asks are listed runtime-wide: every device
   * sees every pending ask. A row whose root is not a stored session (a
   * terminal ask with no engine attached, answered there with /answer) has no
   * thread an answer could start a turn in, so it is not listed.
   */
  function runtimeSessions(db: Database.TxOrDb) {
    return db.select({ id: SessionTable.id }).from(SessionTable)
  }

  /** Pending, unexpired asks: of one thread when `rootSessionID` is given, else of every session on this runtime. */
  function listAsync(rootSessionID?: string): Request[] {
    try {
      const rows = Database.use((db) =>
        db
          .select()
          .from(QuestionAsyncTable)
          .where(
            and(
              eq(QuestionAsyncTable.status, "pending"),
              gte(QuestionAsyncTable.created_at, expiryCutoff()),
              rootSessionID
                ? eq(QuestionAsyncTable.root_session_id, rootSessionID)
                : inArray(QuestionAsyncTable.root_session_id, runtimeSessions(db)),
            ),
          )
          .orderBy(QuestionAsyncTable.created_at)
          .all(),
      )
      return rows.map(asyncRequest)
    } catch (error) {
      // The blocking questions still list when the database can't be read.
      log.error("listing ask_user_later asks failed", { error: error instanceof Error ? error.message : String(error) })
      return []
    }
  }

  /** Pending ask_user_later asks of one thread (its root session). */
  export async function listPendingAsync(rootSessionID: string): Promise<Request[]> {
    return listAsync(rootSessionID)
  }

  /**
   * Persists an ask and publishes it (`question.asked` with `async: true`).
   * Returns at once. Throws AsyncLimitError when the thread already has
   * ASYNC_MAX_PENDING asks waiting.
   */
  export async function askLater(input: { sessionID: string; questions: Info[]; assumption: string }): Promise<Request> {
    await state()
    await expireDue()
    const root = await rootSessionID(input.sessionID)
    const waiting = listAsync(root).length
    if (waiting >= ASYNC_MAX_PENDING) throw new AsyncLimitError(waiting)
    const id = Identifier.ascending("question")
    const created = Date.now()
    const assumption = input.assumption.trim().slice(0, ASYNC_MAX_ASSUMPTION)
    Database.use((db) =>
      db
        .insert(QuestionAsyncTable)
        .values({
          id,
          session_id: input.sessionID,
          root_session_id: root,
          questions: input.questions,
          assumption,
          status: "pending",
          created_at: created,
        })
        .run(),
    )
    log.info("asked later", { id, root, questions: input.questions.length })
    const info: Request = {
      id,
      sessionID: root,
      questions: input.questions,
      async: true,
      assumption,
      time: { created },
    }
    Bus.publish(Event.Asked, info)
    return info
  }

  /** The answer turn's text: marked, one line per question, readable by the model. */
  export function formatAsyncAnswer(request: Pick<Request, "id" | "questions" | "assumption">, answers: Answer[]): string {
    const lines = [`[${ANSWER_MARK} ${request.id}]`, "The user answered the question you asked earlier with ask_user_later:"]
    request.questions.forEach((q, i) => {
      lines.push(`- ${q.question}`, `  → ${formatAnswer(q, answers[i]) || "(no answer)"}`)
    })
    if (request.assumption) {
      lines.push(
        `You had assumed: ${request.assumption}`,
        "If the answer differs, adjust the work you did on that assumption; if it matches, carry on.",
      )
    }
    return lines.join("\n")
  }

  /** Answers an ask_user_later ask: status answered, then a new turn. False when no such pending ask. */
  async function replyAsync(requestID: string, answers: Answer[]): Promise<boolean> {
    // The status guard makes a double reply start one turn, not two.
    const row = Database.use((db) =>
      db
        .update(QuestionAsyncTable)
        .set({ status: "answered", answered_at: Date.now(), answer: { answers } })
        .where(
          and(
            eq(QuestionAsyncTable.id, requestID),
            eq(QuestionAsyncTable.status, "pending"),
            gte(QuestionAsyncTable.created_at, expiryCutoff()),
          ),
        )
        .returning()
        .get(),
    )
    if (!row) return false
    log.info("answered later", { requestID, answers })
    Bus.publish(Event.Replied, { sessionID: row.root_session_id, requestID, answers })
    const text = formatAsyncAnswer(asyncRequest(row), answers)
    void asyncTurn.start({ sessionID: row.root_session_id, requestID, text }).catch((error) =>
      log.error("answer turn failed to start", {
        requestID,
        sessionID: row.root_session_id,
        error: error instanceof Error ? error.message : String(error),
      }),
    )
    return true
  }

  /** Closes a pending ask without a turn (dismissed by the user). False when no such pending ask. */
  async function closeAsync(requestID: string, status: "dismissed"): Promise<boolean> {
    const row = Database.use((db) =>
      db
        .update(QuestionAsyncTable)
        .set({ status })
        .where(and(eq(QuestionAsyncTable.id, requestID), eq(QuestionAsyncTable.status, "pending")))
        .returning()
        .get(),
    )
    if (!row) return false
    log.info("ask_user_later ask closed", { requestID, status })
    Bus.publish(Event.Rejected, { sessionID: row.root_session_id, requestID })
    return true
  }

  /**
   * Expires every ask on this runtime older than ASYNC_EXPIRY_MS, whatever
   * its project (terminal asks included): status expired and
   * `question.rejected` on the sweeping instance's bus, no turn (silent to the
   * model). Returns the ids. The status guard makes a sweep from a second
   * instance a no-op. Listing and replying ignore an expired ask even before a
   * sweep runs.
   */
  export async function expireDue(now = Date.now()): Promise<string[]> {
    const rows = Database.use((db) =>
      db
        .update(QuestionAsyncTable)
        .set({ status: "expired" })
        .where(
          and(
            eq(QuestionAsyncTable.status, "pending"),
            lt(QuestionAsyncTable.created_at, expiryCutoff(now)),
          ),
        )
        .returning()
        .all(),
    )
    for (const row of rows) {
      log.info("ask_user_later ask expired", { requestID: row.id })
      Bus.publish(Event.Rejected, { sessionID: row.root_session_id, requestID: row.id })
    }
    return rows.map((row) => row.id)
  }
}
