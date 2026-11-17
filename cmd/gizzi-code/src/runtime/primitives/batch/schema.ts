/**
 * The output schema for agent_batch: up to 20 flat fields, each a string,
 * number, boolean or one of a fixed list (enum). Each worker replies with one
 * JSON object; validateReply checks it and gives the error sentence the
 * worker gets back for its one retry.
 *
 * Free of runtime state, so the Ink adapter and bun tests can use it.
 */
import z from "zod/v4"

export const MAX_FIELDS = 20
export const FIELD_TYPES = ["string", "number", "boolean", "enum"] as const
export type FieldType = (typeof FIELD_TYPES)[number]

/** Letters, digits and underscores, starting with a letter. A leading `_` is kept for `_status` and `_error`. */
export const FIELD_NAME = /^[A-Za-z][A-Za-z0-9_]{0,63}$/

export const field = z.object({
  name: z.string().regex(FIELD_NAME, "Field names use letters, digits and _, start with a letter, at most 64 characters"),
  type: z.enum(FIELD_TYPES).describe("string, number, boolean, or enum (one of `values`)"),
  description: z.string().max(300).optional().describe("What the worker should put in this field"),
  values: z.array(z.string().min(1).max(100)).min(1).max(50).optional().describe("For enum: the allowed values"),
  optional: z.boolean().optional().describe("true: the worker may answer null when it can't fill this field"),
})

export type Field = z.infer<typeof field>

export const outputSchema = z
  .array(field)
  .min(1)
  .max(MAX_FIELDS)
  .superRefine((fields, ctx) => {
    const seen = new Set<string>()
    fields.forEach((f, i) => {
      const key = f.name.toLowerCase()
      if (seen.has(key)) ctx.addIssue({ code: "custom", path: [i, "name"], message: `Field "${f.name}" is listed twice` })
      seen.add(key)
      if (f.type === "enum" && !f.values?.length) {
        ctx.addIssue({ code: "custom", path: [i, "values"], message: `Enum field "${f.name}" needs its allowed values` })
      }
      if (f.type !== "enum" && f.values) {
        ctx.addIssue({ code: "custom", path: [i, "values"], message: `Only enum fields take values ("${f.name}" is ${f.type})` })
      }
    })
  })
  .describe(`The columns each worker fills: 1 to ${MAX_FIELDS} flat fields`)

export type OutputSchema = z.infer<typeof outputSchema>

export type CellValue = string | number | boolean | null
export type ValidReply = { ok: true; values: Record<string, CellValue> }
export type InvalidReply = { ok: false; error: string }

/** The reply format the worker is told to use, appended to its prompt. */
export function replyInstructions(fields: readonly Field[]): string {
  const lines = fields.map((f) => {
    const kind =
      f.type === "enum" ? `one of ${f.values!.map((v) => JSON.stringify(v)).join(", ")}` : f.type
    const need = f.optional ? "optional, null when unknown" : "required"
    return `- ${f.name} (${kind}, ${need})${f.description ? `: ${f.description}` : ""}`
  })
  return [
    "Reply with only one JSON object with these fields, and nothing else: no prose, no code fence.",
    ...lines,
  ].join("\n")
}

/** The follow-up a worker gets when its reply did not match. */
export function retryInstructions(error: string): string {
  return `Your reply did not match the required format: ${error}\nReply again with only the corrected JSON object.`
}

/** The JSON object in a worker's reply: the whole text, a fenced block, or the outermost {...}. */
export function extractJson(text: string): unknown {
  const trimmed = text.trim()
  const attempts = [trimmed]
  const fence = /```(?:json)?\s*\n?([\s\S]*?)```/i.exec(trimmed)
  if (fence) attempts.push(fence[1]!.trim())
  const first = trimmed.indexOf("{")
  const last = trimmed.lastIndexOf("}")
  if (first >= 0 && last > first) attempts.push(trimmed.slice(first, last + 1))
  for (const candidate of attempts) {
    try {
      return JSON.parse(candidate)
    } catch {}
  }
  throw new Error("the reply is not valid JSON.")
}

/**
 * Check a worker's reply against the schema. Extra keys are ignored (the
 * output has only the schema's columns); a missing required field, a wrong
 * type or a value outside an enum is an error.
 */
export function validateReply(fields: readonly Field[], text: string): ValidReply | InvalidReply {
  let parsed: unknown
  try {
    parsed = extractJson(text)
  } catch (error) {
    return { ok: false, error: (error as Error).message }
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { ok: false, error: "the reply must be one JSON object, not an array or a single value." }
  }
  const obj = parsed as Record<string, unknown>
  const problems: string[] = []
  const values: Record<string, CellValue> = {}
  for (const f of fields) {
    const v = obj[f.name]
    if (v === undefined || v === null) {
      if (f.optional) values[f.name] = null
      else problems.push(`"${f.name}" is missing`)
      continue
    }
    switch (f.type) {
      case "string":
        if (typeof v !== "string") problems.push(`"${f.name}" must be a string`)
        else values[f.name] = v
        break
      case "number":
        if (typeof v !== "number" || !Number.isFinite(v)) problems.push(`"${f.name}" must be a number`)
        else values[f.name] = v
        break
      case "boolean":
        if (typeof v !== "boolean") problems.push(`"${f.name}" must be true or false`)
        else values[f.name] = v
        break
      case "enum":
        if (typeof v !== "string" || !f.values!.includes(v)) {
          problems.push(`"${f.name}" must be one of ${f.values!.map((x) => JSON.stringify(x)).join(", ")}`)
        } else values[f.name] = v
        break
    }
  }
  if (problems.length) return { ok: false, error: problems.join("; ") + "." }
  return { ok: true, values }
}
