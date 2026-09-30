export const GRAPH_ID = "coding.bug_fix.v1"
export const TEMPLATE_ID = "BUG_FIX"
export const CRITERIA = ["target_tests_pass", "affected_tests_pass", "no_new_regressions", "diff_review_accept", "requirements_satisfied"] as const
export type Category = "off-by-one" | "null-handling" | "wrong-import" | "async-await" | "sort-comparator" | "falsy-default"
export type Files = Record<string, string>
/** Private fields stay in the evaluator; runners receive only Task. */
export interface Fixture {
  id: string; seed: number; category: Category; report: string; files: Files
  truth: { file: string; fixedSource: string; patch: string; explanation: string }
}
export interface Budget { maxTokens: number; maxCalls: number; timeoutMs: number }
export interface Task {
  fixtureId: string; report: string; repo: string; files: Files; editablePaths: string[]
  backendId: string; budget: Budget
}
export interface Usage {
  input: number; output: number; reasoning: number; cacheRead: number; cacheWrite: number; estimated: boolean
}
export interface Completion { patch: string; text: string; usage: Usage }
export interface ModelRequest {
  backendId: string; report: string; files: Files; feedback?: string; attempt: number; budget: Budget
  /** Production graph nodes may request hypotheses/reviews rather than a patch. */
  stage?: string; instruction?: string; responseFormat?: "patch" | "text"
}
export interface Backend {
  id: string; kind: "mock" | "http"
  complete(request: ModelRequest): Promise<Completion>
}
export interface Check { passed: boolean; exitCode: number; output: string }
export interface Verification { target: Check; regression: Check; integrity: boolean }
export interface GraphEvidence {
  runId: string; runReceiptId: string; mutationReceiptIds: string[]; verificationReceiptIds: string[]
  verifierId: string; workerId: string; verdict: "PASS" | "FAIL"
  criteria: Record<(typeof CRITERIA)[number], boolean>
  /** Receipt resolution and identity checks are performed by the production adapter. */
  receiptsValidated: boolean; completionOwner: "verifier"
  /** All additional cognitive usage was recorded through context.accountUsage. */
  telemetryComplete: boolean
}
export interface RunOutput {
  usage: Usage | null; calls: number; evidence: GraphEvidence | null; patchAccepted: boolean; error?: string
}
export interface Runner { mode: "naked" | "system"; run(task: Task): Promise<RunOutput> }
export interface GraphContext {
  task: Task
  signal: AbortSignal
  /** A budgeted handle to the SAME backend as the naked mode. */
  model: Backend
  /** Charge non-generator cognition (decision/verifier work) to the same total budget. */
  accountUsage(usage: Usage): void
  verify(): Promise<Verification>
  applyPatch(patch: string): Promise<boolean>
}
export interface BugFixGraph {
  id: typeof GRAPH_ID; revision: string; production: boolean
  /** Production integration must attest WP10 landed and WP12 passed. */
  gates: { wp10: string | null; wp12: string | null }
  execute(context: GraphContext): Promise<GraphEvidence | null>
}
export const emptyUsage = (): Usage => ({ input: 0, output: 0, reasoning: 0, cacheRead: 0, cacheWrite: 0, estimated: false })
export const tokenCount = (u: Usage) => u.input + u.output + u.reasoning + u.cacheRead + u.cacheWrite
export function addUsage(a: Usage, b: Usage): Usage {
  return { input: a.input + b.input, output: a.output + b.output, reasoning: a.reasoning + b.reasoning,
    cacheRead: a.cacheRead + b.cacheRead, cacheWrite: a.cacheWrite + b.cacheWrite, estimated: a.estimated || b.estimated }
}
