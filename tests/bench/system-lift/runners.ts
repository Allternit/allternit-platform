import { performance } from "node:perf_hooks"
import { CRITERIA, GRAPH_ID, addUsage, emptyUsage, tokenCount,
  type Backend, type BugFixGraph, type GraphContext, type GraphEvidence, type ModelRequest,
  type Runner, type RunOutput, type Task } from "./types"
import { applyPatch, sourceFiles } from "./workspace"

function meter(task: Task, backend: Backend, callLimit = task.budget.maxCalls) {
  const start = performance.now()
  let usage = emptyUsage(), calls = 0, telemetryComplete = true
  function check() {
    if (performance.now() - start >= task.budget.timeoutMs) throw new Error("wall-time budget exceeded")
    if (tokenCount(usage) >= task.budget.maxTokens) throw new Error("token budget exhausted")
  }
  function accountUsage(extra: ReturnType<typeof emptyUsage>) {
    if (!extra || typeof extra.estimated !== "boolean" ||
      [extra.input, extra.output, extra.reasoning, extra.cacheRead, extra.cacheWrite].some(value => !Number.isFinite(value) || value < 0)) {
      telemetryComplete = false; throw new Error("invalid token telemetry")
    }
    usage = addUsage(usage, extra)
    if (tokenCount(usage) > task.budget.maxTokens) throw new Error("token budget exceeded")
  }
  const model: Backend = {
    id: backend.id, kind: backend.kind,
    async complete(request: ModelRequest) {
      check()
      if (request.backendId !== task.backendId || backend.id !== task.backendId) throw new Error("backend substitution forbidden")
      if (calls >= callLimit) throw new Error("model-call budget exhausted")
      calls++
      let result
      try {
        result = await backend.complete({ ...request, budget: { ...task.budget,
          maxTokens: task.budget.maxTokens - tokenCount(usage), timeoutMs: Math.max(1, task.budget.timeoutMs - Math.ceil(performance.now() - start)) } })
      } catch (error) { telemetryComplete = false; throw error }
      accountUsage(result.usage)
      if (performance.now() - start >= task.budget.timeoutMs) throw new Error("wall-time budget exceeded")
      return result
    },
  }
  return { model, check, accountUsage, output: () => ({ usage: telemetryComplete ? usage : null, calls }) }
}

export class NakedRunner implements Runner {
  readonly mode = "naked" as const
  constructor(private readonly backend: Backend) {}
  async run(task: Task): Promise<RunOutput> {
    const m = meter(task, this.backend, 1)
    let patchAccepted = false, error: string | undefined
    try {
      const result = await m.model.complete({ backendId: task.backendId, files: task.files, report: task.report, attempt: 1, budget: task.budget })
      m.check()
      patchAccepted = await applyPatch(task.repo, result.patch)
    } catch (e) { error = String(e) }
    return { ...m.output(), patchAccepted, evidence: null, error }
  }
}

export class SystemRunner implements Runner {
  readonly mode = "system" as const
  constructor(private readonly backend: Backend, readonly graph: BugFixGraph, private readonly verify: (task: Task) => ReturnType<GraphContext["verify"]>) {
    if (graph.id !== GRAPH_ID) throw new Error("system mode requires canonical BUG_FIX graph")
  }
  async run(task: Task): Promise<RunOutput> {
    const m = meter(task, this.backend)
    let evidence: GraphEvidence | null = null, patchAccepted = false, error: string | undefined
    try {
      evidence = await this.graph.execute({ task, signal: AbortSignal.timeout(task.budget.timeoutMs), model: m.model, accountUsage: m.accountUsage,
        verify: async () => { m.check(); return this.verify(task) },
        applyPatch: async patch => { m.check(); const accepted = await applyPatch(task.repo, patch); patchAccepted ||= accepted; return accepted },
      })
      m.check()
    } catch (e) { error = String(e) }
    const output = m.output()
    if (this.graph.production && evidence?.telemetryComplete !== true) output.usage = null
    return { ...output, patchAccepted, evidence, error }
  }
}

export function evidencePass(e: GraphEvidence | null): boolean {
  return !!e && e.verdict === "PASS" && e.completionOwner === "verifier" && e.receiptsValidated === true &&
    [e.runId, e.runReceiptId, e.verifierId, e.workerId].every(id => typeof id === "string" && id.trim().length > 0) && e.verifierId !== e.workerId &&
    Array.isArray(e.mutationReceiptIds) && e.mutationReceiptIds.length > 0 && e.mutationReceiptIds.every(id => typeof id === "string" && !!id) &&
    Array.isArray(e.verificationReceiptIds) && e.verificationReceiptIds.length > 0 && e.verificationReceiptIds.every(id => typeof id === "string" && !!id) &&
    CRITERIA.every(c => e.criteria?.[c] === true)
}

/** Offline wiring exercise only. It is not WP10 and can never satisfy eligibility. */
export class MockBugFixGraph implements BugFixGraph {
  readonly id = GRAPH_ID
  readonly revision = "offline-harness-v1"
  readonly production = false
  readonly gates = { wp10: null, wp12: null }
  async execute(ctx: GraphContext): Promise<GraphEvidence> {
    let feedback: string | undefined
    for (let attempt = 1; attempt <= ctx.task.budget.maxCalls; attempt++) {
      const completion = await ctx.model.complete({ backendId: ctx.task.backendId, files: {
        ...ctx.task.files, ...await sourceFiles(ctx.task.repo),
      }, report: ctx.task.report, feedback, attempt, budget: ctx.task.budget })
      const accepted = await ctx.applyPatch(completion.patch)
      const verification = await ctx.verify()
      if (accepted && verification.integrity && verification.target.passed && verification.regression.passed) {
        const runId = `mock-run:${ctx.task.fixtureId}`
        return { runId, runReceiptId: `${runId}:run-receipt`, mutationReceiptIds: [`${runId}:mutation`],
          verificationReceiptIds: [`${runId}:verification`], verifierId: "mock-verifier", workerId: "mock-worker",
          verdict: "PASS", criteria: Object.fromEntries(CRITERIA.map(c => [c, true])) as GraphEvidence["criteria"],
          receiptsValidated: true, completionOwner: "verifier", telemetryComplete: true }
      }
      feedback = verification.target.output + "\n" + verification.regression.output
    }
    throw new Error("mock graph repair budget exhausted")
  }
}
