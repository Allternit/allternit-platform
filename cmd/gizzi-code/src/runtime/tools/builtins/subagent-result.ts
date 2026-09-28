/**
 * The compressed subagent result (spec P7.1): what a parent thread shows as a
 * subagent card — summary, confidence, open questions, artifacts — with the
 * full transcript one click away (the child session). Subagents are asked to
 * end with a small JSON block; without one, the result is derived from the
 * reply and the files the subagent wrote.
 */
import { MessageV2 } from "@/runtime/session/message-v2"

export interface SubagentResult {
  summary: string
  confidence?: number
  openQuestions: string[]
  artifacts: string[]
}

export const RESULT_INSTRUCTION = [
  "",
  "When you finish, end your final reply with a result block the parent can read at a glance:",
  "```json",
  '{"result": {"summary": "<the answer in 1-2 sentences, exact figures>", "confidence": <0-1>, "openQuestions": ["<what is still open>"], "artifacts": ["<files or links you produced>"]}}',
  "```",
].join("\n")

const list = (x: unknown) => (Array.isArray(x) ? x.filter((i): i is string => typeof i === "string" && i.trim() !== "") : [])

/** Parse the trailing result block, and the reply without it. */
export function parseResultBlock(text: string): { result?: SubagentResult; body: string } {
  const re = /```(?:json)?\s*(\{[\s\S]*?"result"[\s\S]*?\})\s*```\s*$/
  const m = text.match(re)
  if (!m) return { body: text }
  try {
    const r = JSON.parse(m[1]).result
    if (!r || typeof r.summary !== "string" || !r.summary.trim()) return { body: text }
    const confidence = typeof r.confidence === "number" && r.confidence >= 0 && r.confidence <= 1 ? r.confidence : undefined
    return {
      result: { summary: r.summary.trim(), confidence, openQuestions: list(r.openQuestions), artifacts: list(r.artifacts) },
      body: text.slice(0, m.index).trimEnd(),
    }
  } catch {
    return { body: text }
  }
}

/** Files the subagent wrote or edited, from its tool calls. */
export function writtenFiles(messages: MessageV2.WithParts[]): string[] {
  const out = new Set<string>()
  for (const m of messages) {
    for (const p of m.parts) {
      if (p.type !== "tool" || p.state.status !== "completed") continue
      if (!["write", "edit", "apply_patch", "multiedit"].includes(p.tool)) continue
      const file = (p.state.input as Record<string, unknown>)?.filePath ?? (p.state.input as Record<string, unknown>)?.path
      if (typeof file === "string") out.add(file)
    }
  }
  return [...out]
}

/** The card for a finished subagent: its own block, else derived. */
export function subagentResult(text: string, messages: MessageV2.WithParts[]): { result: SubagentResult; body: string } {
  const parsed = parseResultBlock(text)
  const files = writtenFiles(messages)
  if (parsed.result) {
    const artifacts = [...new Set([...parsed.result.artifacts, ...files])]
    return { result: { ...parsed.result, artifacts }, body: parsed.body }
  }
  const sentences = text.replace(/\s+/g, " ").trim().split(/(?<=[.!?])\s+/)
  const summary = sentences.slice(0, 2).join(" ").slice(0, 400)
  const openQuestions = sentences.filter((s) => s.endsWith("?")).slice(0, 3)
  return { result: { summary: summary || "(no reply)", openQuestions, artifacts: files }, body: text }
}
