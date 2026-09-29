/**
 * D16 human gate for Subscription Fabric work nobody sent from the chat
 * composer.
 *
 * A chat send carries the human action the chat bridge minted when the
 * person pressed send. Anything else — a task an agent, tool, subagent, bot
 * or schedule prepared — and every provider question from a running task,
 * goes to a person as a `subscription` permission ask. That class always
 * asks, in every permission mode (PermissionNext.ALWAYS_ASK); nothing here or
 * in the permission policy can answer it. When the person approves the card
 * in the Allternit app, allternit-api mints the human action server-side and
 * relays it with the reply. A reply without one (a terminal UI, a remote
 * peer) cannot start a task: gizzi never mints actions itself.
 */

import { Identifier } from "@/shared/id/id"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { FabricError, providerDisplayName } from "./client"

export const SUBSCRIPTION_PERMISSION = "subscription"
/** How much of a prepared prompt the card shows. */
const PROMPT_PREVIEW_CHARS = 4000

export interface SendConfirmation {
  sessionID: string
  provider: string
  modelClass: string
  prompt: string
  signal?: AbortSignal
}

export interface QuestionInput {
  sessionID: string
  provider: string
  taskID: string
  question: string
  reason?: string
  signal?: AbortSignal
}

/** Ask the person to confirm a prepared fabric task. Returns its human action. */
export async function confirmSend(input: SendConfirmation): Promise<string> {
  const name = providerDisplayName(input.provider)
  const data = await ask(input.sessionID, input.provider, input.signal, {
    kind: "send",
    provider: input.provider,
    providerName: name,
    model: input.modelClass,
    prompt: input.prompt.slice(0, PROMPT_PREVIEW_CHARS),
    truncated: input.prompt.length > PROMPT_PREVIEW_CHARS,
  }, `Send to your ${name} subscription`, () => `You chose not to send this to your ${name} subscription.`)
  return requireAction(data, name)
}

/**
 * Put a provider's mid-task question to the person. Returns their answer and
 * the human action for sending it, or throws when they decline.
 */
export async function askProviderQuestion(input: QuestionInput): Promise<{ humanAction: string; answer: string }> {
  const name = providerDisplayName(input.provider)
  const data = await ask(input.sessionID, input.provider, input.signal, {
    kind: "question",
    provider: input.provider,
    providerName: name,
    taskId: input.taskID,
    question: input.question,
    ...(input.reason ? { reason: input.reason } : {}),
  }, `${name} asks: ${input.question}`, () => `${name} needs you: ${input.question}`)
  const humanAction = requireAction(data, name)
  const answer = data?.answer?.trim()
  if (!answer) throw new FabricError(`${name} needs you: ${input.question}`, 409, "answer_required")
  return { humanAction, answer }
}

async function ask(
  sessionID: string,
  provider: string,
  signal: AbortSignal | undefined,
  subscription: Record<string, unknown>,
  summary: string,
  declined: () => string,
): Promise<PermissionNext.ReplyData | undefined> {
  if (signal?.aborted) throw new DOMException("aborted", "AbortError")
  // A subagent's session is not the one the person is looking at: put the
  // card on the conversation that started it.
  sessionID = await rootSessionID(sessionID)
  const id = Identifier.ascending("permission")
  // A turn stopped while the card is open withdraws the card.
  const withdraw = () => void PermissionNext.reply({ requestID: id, reply: "reject" }).catch(() => {})
  signal?.addEventListener("abort", withdraw, { once: true })
  try {
    return await PermissionNext.ask({
      id,
      sessionID,
      permission: SUBSCRIPTION_PERMISSION,
      patterns: [provider],
      always: [],
      metadata: { subscription, summary },
      ruleset: [],
    })
  } catch (error) {
    if (signal?.aborted) throw new DOMException("aborted", "AbortError")
    if (error instanceof PermissionNext.DeniedError) {
      throw new FabricError(
        "Subscription models need a person to confirm each task, and this session cannot ask one.",
        403,
        "human_action_required",
      )
    }
    if (error instanceof PermissionNext.RejectedError || error instanceof PermissionNext.CorrectedError) {
      throw new FabricError(declined(), 403, "declined")
    }
    throw error
  } finally {
    signal?.removeEventListener("abort", withdraw)
  }
}

/** The top-level session a (possibly nested) subagent session belongs to. */
export async function rootSessionID(sessionID: string): Promise<string> {
  try {
    // Relative dynamic import: session → providers is a static cycle, and the
    // production bundler does not apply tsconfig paths to dynamic imports.
    const { Session } = await import("../../session")
    let current = sessionID
    for (let depth = 0; depth < 16; depth++) {
      const info = await Session.get(current).catch(() => undefined)
      if (!info?.parentID) return current
      current = info.parentID
    }
    return current
  } catch {
    return sessionID
  }
}

function requireAction(data: PermissionNext.ReplyData | undefined, name: string): string {
  if (data?.humanAction) return data.humanAction
  throw new FabricError(
    `Confirm ${name} subscription tasks in the Allternit app. It records your confirmation; this reply did not include one.`,
    403,
    "human_action_required",
  )
}
