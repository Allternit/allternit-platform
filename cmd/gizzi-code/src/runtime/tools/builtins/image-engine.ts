/**
 * Image engine resolver. Pure and dependency-injected: engines are passed in as
 * functions, no spawning, no PaneRender calls. media_generate wires the real engines.
 *
 * Failure rule:
 *   not_started -> next engine, quietly
 *   definite    -> stop, plain error
 *   ambiguous   -> stop, "Check before trying again", never retried, never falls through
 */

export const MAX_ENGINE_PROMPT = 16_000

export type EngineId = "codex" | "chatgpt_browser"

export type EngineProgress = (label: string) => void

export type EngineResult =
  | { status: "ok"; image: string; width: number; height: number }
  | { status: "not_started"; reason?: string }
  | {
      status: "definite"
      error: string
      /** Result code, e.g. "usage_limit", "invalid_image", "sign_in_needed". */
      code?: string
      /** usage_limit only: epoch ms when the engine works again. */
      resetsAt?: number
    }
  | { status: "ambiguous"; error?: string }

export type EngineInput = { prompt: string; signal?: AbortSignal; progress?: EngineProgress }

export type ImageEngine = { id: EngineId; run: (input: EngineInput) => Promise<EngineResult> }

export type ImageOutcome =
  | { kind: "image"; engine: EngineId; image: string; width: number; height: number }
  /** Every engine was not_started: tell the model to draw it in code and say so. */
  | { kind: "no_engine" }
  | { kind: "error"; engine?: EngineId; code: string; message: string }
  | { kind: "ambiguous"; engine: EngineId; code: "ambiguous"; message: string }

export const AMBIGUOUS_MESSAGE = "Check before trying again. The picture may already have been made."

const IMAGE_URL = /^data:image\/(png|gif);base64,[A-Za-z0-9+/]+={0,2}$/

// Module-level state. Keyed by engine; value is the epoch ms the engine is usable again.
const cooldowns = new Map<EngineId, number>()
const claimed = new Set<string>()

export function resetImageEngineState() {
  cooldowns.clear()
  claimed.clear()
}

/** True the first time a user message asks for an engine job; later claims return false (caller returns already_used). */
export function claimImageJob(userMessageId: string): boolean {
  if (claimed.has(userMessageId)) return false
  claimed.add(userMessageId)
  return true
}

/** The brief verbatim, then earlier briefs from this chat as labelled context (newest last), total <= 16000 chars. */
export function buildEnginePrompt(brief: string, earlierBriefs: readonly string[] = []): string {
  if (brief.length >= MAX_ENGINE_PROMPT) return brief.slice(0, MAX_ENGINE_PROMPT)
  const header = "\n\nEarlier picture requests in this chat, oldest first. Context only; do not redraw them:"
  const lines = earlierBriefs.map((b) => b.trim()).filter(Boolean).map((b) => `\n- ${b}`)
  // Drop oldest first until it fits; newest stays last.
  while (lines.length && brief.length + header.length + lines.join("").length > MAX_ENGINE_PROMPT) lines.shift()
  return lines.length ? brief + header + lines.join("") : brief
}

export async function resolveImageJob(input: {
  engines: readonly ImageEngine[]
  prompt: string
  earlierBriefs?: readonly string[]
  signal?: AbortSignal
  progress?: EngineProgress
  now: () => number
}): Promise<ImageOutcome> {
  const prompt = buildEnginePrompt(input.prompt, input.earlierBriefs)
  for (const engine of input.engines) {
    if (input.signal?.aborted) return { kind: "error", code: "cancelled", message: "Cancelled." }
    const until = cooldowns.get(engine.id)
    if (until !== undefined) {
      if (input.now() < until) continue // treated as not_started
      cooldowns.delete(engine.id)
    }
    let result: EngineResult
    try {
      result = await engine.run({ prompt, signal: input.signal, progress: input.progress })
    } catch {
      // Unknown whether the job began: never retry or fall through.
      result = { status: "ambiguous" }
    }
    switch (result.status) {
      case "not_started":
        continue
      case "ok":
        if (!IMAGE_URL.test(result.image) || !(result.width > 0) || !(result.height > 0))
          return { kind: "error", engine: engine.id, code: "invalid_image", message: "The picture engine returned an image that could not be used." }
        return { kind: "image", engine: engine.id, image: result.image, width: result.width, height: result.height }
      case "definite":
        if (result.code === "usage_limit" && typeof result.resetsAt === "number") cooldowns.set(engine.id, result.resetsAt)
        return { kind: "error", engine: engine.id, code: result.code ?? "unavailable", message: result.error }
      case "ambiguous":
        return { kind: "ambiguous", engine: engine.id, code: "ambiguous", message: result.error ? `${result.error} ${AMBIGUOUS_MESSAGE}` : AMBIGUOUS_MESSAGE }
    }
  }
  return { kind: "no_engine" }
}
