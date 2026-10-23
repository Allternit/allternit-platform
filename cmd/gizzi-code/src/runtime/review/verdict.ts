import z from "zod/v4"

/** The review verdict contract. Pure: no runtime imports, so the CLI, the
 * server routes and the tests can all use it without booting an Instance. */
export namespace ReviewVerdict {
  export const Kind = z.enum(["PASS", "FAIL", "PARTIAL"])
  export type Kind = z.infer<typeof Kind>

  export const Finding = z.object({
    file: z.string().min(1).describe("Repo-relative path of the file with the problem"),
    line: z.number().int().positive().optional().describe("1-based line number when known"),
    problem: z.string().min(1).describe("What is wrong, concretely"),
  })
  export type Finding = z.infer<typeof Finding>

  export const Unverified = z.object({
    what: z.string().min(1).describe("The claim or behaviour that could not be checked"),
    why: z.string().min(1).describe("Why it could not be checked"),
  })
  export type Unverified = z.infer<typeof Unverified>

  /** Shape the reviewer must return (also sent to the model as JSON schema). */
  export const Shape = z.object({
    verdict: Kind,
    verified: z.array(z.string()).describe("Things you checked and confirmed, with the evidence"),
    findings: z.array(Finding).describe("Problems found. Required (≥1) when verdict is FAIL"),
    unverified: z.array(Unverified).describe("Things you could not verify. Required (≥1) when verdict is PARTIAL"),
    summary: z.string().describe("One or two sentences for the user"),
  })
  export type Shape = z.infer<typeof Shape>

  /** Shape plus the verdict rules (FAIL ⇒ findings, PARTIAL ⇒ unverified). */
  export const Output = Shape.superRefine((value, ctx) => {
    if (value.verdict === "FAIL" && value.findings.length === 0) {
      ctx.addIssue({ code: "custom", path: ["findings"], message: "FAIL needs at least one finding with a file" })
    }
    if (value.verdict === "PARTIAL" && value.unverified.length === 0) {
      ctx.addIssue({ code: "custom", path: ["unverified"], message: "PARTIAL needs at least one unverified item" })
    }
  })

  export function jsonSchema(): Record<string, unknown> {
    const schema = z.toJSONSchema(Shape) as Record<string, unknown>
    delete schema["$schema"]
    return schema
  }

  /** Loose cleanup before validation: models often send `line: null`,
   * a string line number, or a JSON string instead of an object. */
  function normalize(raw: unknown): unknown {
    let value = raw
    if (typeof value === "string") {
      const text = value.trim().replace(/^```(?:json)?\s*/i, "").replace(/```$/, "").trim()
      try {
        value = JSON.parse(text)
      } catch {
        return raw
      }
    }
    if (!value || typeof value !== "object" || Array.isArray(value)) return value
    const record = { ...(value as Record<string, unknown>) }
    if (typeof record.verdict === "string") record.verdict = record.verdict.trim().toUpperCase()
    for (const key of ["verified", "findings", "unverified"] as const) {
      if (record[key] === undefined || record[key] === null) record[key] = []
    }
    if (record.summary === undefined || record.summary === null) record.summary = ""
    if (Array.isArray(record.findings)) {
      record.findings = record.findings.map((item) => {
        if (!item || typeof item !== "object") return item
        const finding = { ...(item as Record<string, unknown>) }
        if (finding.line === null || finding.line === "" || finding.line === 0) delete finding.line
        else if (typeof finding.line === "string" && /^\d+$/.test(finding.line)) finding.line = Number(finding.line)
        return finding
      })
    }
    return record
  }

  export type ParseResult = { ok: true; value: Shape } | { ok: false; error: string }

  export function parse(raw: unknown): ParseResult {
    if (raw === undefined || raw === null) return { ok: false, error: "The reviewer returned no structured output" }
    const result = Output.safeParse(normalize(raw))
    if (result.success) return { ok: true, value: result.data }
    const error = result.error.issues
      .map((issue) => `${issue.path.join(".") || "(root)"}: ${issue.message}`)
      .join("; ")
    return { ok: false, error }
  }

  /** The verdict used when the reviewer could not produce a valid one. */
  export function partialFromFailure(reason: string): Shape {
    return {
      verdict: "PARTIAL",
      verified: [],
      findings: [],
      unverified: [{ what: "The review itself", why: reason }],
      summary: `The reviewer did not return a valid verdict: ${reason}`,
    }
  }

  /** Validate, ask the reviewer to repair once, then fall back to PARTIAL. */
  export async function parseWithRepair(
    raw: unknown,
    repair: (error: string) => Promise<unknown>,
  ): Promise<{ value: Shape; repaired: boolean; failed: boolean }> {
    const first = parse(raw)
    if (first.ok) return { value: first.value, repaired: false, failed: false }
    let second: ParseResult
    try {
      second = parse(await repair(first.error))
    } catch (error) {
      second = { ok: false, error: error instanceof Error ? error.message : String(error) }
    }
    if (second.ok) return { value: second.value, repaired: true, failed: false }
    return { value: partialFromFailure(second.error), repaired: true, failed: true }
  }

  /** A stored verdict: the model's answer plus what the runtime knows. */
  export const Record = Shape.extend({
    id: z.string(),
    sessionID: z.string().optional(),
    reviewerSessionID: z.string().optional(),
    time: z.object({ started: z.number(), finished: z.number() }),
    model: z.string().optional(),
    source: z.enum(["session", "git"]),
    files: z.array(z.object({ path: z.string(), status: z.enum(["added", "modified", "deleted"]) })),
    instruction: z.string().optional(),
    round: z.number().int().positive().optional(),
    cost: z.number().optional(),
    tokens: z.object({ input: z.number(), output: z.number(), reasoning: z.number().optional() }).optional(),
  })
  export type Record = z.infer<typeof Record>

  /** `gizzi run --review` exit codes: 0 PASS / nothing to review, 3 FAIL, 4 PARTIAL. */
  export function exitCode(verdict: Kind | null | undefined): number {
    if (verdict === "FAIL") return 3
    if (verdict === "PARTIAL") return 4
    return 0
  }

  export function formatFindings(findings: Finding[]): string {
    return findings.map((f) => `- ${f.file}${f.line ? `:${f.line}` : ""} — ${f.problem}`).join("\n")
  }

  /** Human-readable rendering for the CLI and the TUI. */
  export function format(record: Pick<Record, "verdict" | "summary" | "verified" | "findings" | "unverified"> & { model?: string; files?: Record["files"] }): string {
    const lines = [`Review: ${record.verdict}${record.model ? ` (${record.model})` : ""}`]
    if (record.summary) lines.push(record.summary)
    if (record.files?.length) lines.push(`Files: ${record.files.length}`)
    if (record.findings.length) lines.push("", "Findings:", formatFindings(record.findings))
    if (record.unverified.length) {
      lines.push("", "Not verified:", ...record.unverified.map((u) => `- ${u.what} — ${u.why}`))
    }
    if (record.verified.length) lines.push("", "Verified:", ...record.verified.map((v) => `- ${v}`))
    return lines.join("\n")
  }

  /** Stable fingerprint of a non-passing result, used to detect "no progress". */
  export function fingerprint(record: Pick<Shape, "verdict" | "findings" | "unverified">, extra: string[] = []): string {
    const parts = [
      record.verdict,
      ...record.findings.map((f) => `${f.file}:${f.line ?? ""}:${f.problem.trim().toLowerCase()}`).sort(),
      ...record.unverified.map((u) => `?${u.what.trim().toLowerCase()}`).sort(),
      ...[...extra].sort(),
    ]
    return parts.join("\n")
  }
}
