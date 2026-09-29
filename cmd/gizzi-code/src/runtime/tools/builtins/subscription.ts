/**
 * Subscription capability tools (SURFACES_PLAN §3 step 5): the tool belt's
 * way to make things with the user's connected subscriptions (ChatGPT,
 * Claude, Kimi) on their Sessions computer — presentations, documents, deep
 * research, and images through `media_generate`'s subscription lane.
 *
 * D16 is structural here:
 * - Every call asks the person with the `subscription` permission, which is
 *   always-ask in every mode (PermissionNext.ALWAYS_ASK).
 * - The task is submitted only with the human action the confirming surface
 *   attached to that approval. A tool never mints or invents one: no human
 *   action on the grant → nothing is sent.
 * - A question the provider asks mid-task is handed to the person; it is
 *   never answered here.
 */

import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import type { MessageV2 } from "@/runtime/session/message-v2"
import { fabricConfigured } from "@/runtime/providers/fabric/client"
import {
  availableProviders,
  cancelFabricTask,
  dataUrl,
  downloadFabricArtifact,
  fabricCapabilities,
  followFabricTask,
  submitFabricTask,
  type FabricTask,
} from "@/runtime/providers/fabric/tasks"

export const SUBSCRIPTION_PERMISSION = "subscription"

const PROVIDER_NAMES: Record<string, string> = { chatgpt: "ChatGPT", claude: "Claude", kimi: "Kimi" }
export const providerName = (provider: string) => PROVIDER_NAMES[provider] ?? provider

export class SubscriptionNotConfirmedError extends Error {
  constructor(provider: string) {
    super(
      `Nothing was sent to ${providerName(provider)}. Subscription tasks run only after you confirm them in the Allternit app, ` +
        "and this confirmation did not come from there. Ask the user to confirm the task in the app.",
    )
    this.name = "SubscriptionNotConfirmedError"
  }
}

export interface SubscriptionTaskInput {
  capability: string
  provider: string
  prompt: string
  title: string
  /** What the person is confirming, in plain words (shown on the card). */
  summary: string
  options?: Record<string, unknown>
}

/**
 * Ask the person to confirm one subscription task and return the human
 * action their confirmation carried. Throws when the ask is refused or the
 * reply carries no human action (e.g. a context with nobody to ask).
 */
export async function confirmSubscriptionTask(ctx: Tool.Context, input: SubscriptionTaskInput): Promise<string> {
  const grant = await ctx.ask({
    permission: SUBSCRIPTION_PERMISSION,
    patterns: [`${input.provider}:${input.capability}`],
    // Never offer "always": each task is its own human act.
    always: [],
    // Same card shape as the fabric model's own sends (human-gate.ts): the
    // platform renders `subscription` and shows `summary`.
    metadata: {
      toolName: input.capability,
      summary: input.summary,
      subscription: {
        kind: "send",
        provider: input.provider,
        providerName: providerName(input.provider),
        capability: input.capability,
        title: input.title,
        prompt: input.prompt.slice(0, 2_000),
        truncated: input.prompt.length > 2_000,
      },
    },
  })
  const humanAction = grant && typeof grant === "object" ? grant.humanAction?.trim() : undefined
  if (!humanAction) throw new SubscriptionNotConfirmedError(input.provider)
  return humanAction
}

export interface SubscriptionTaskResult {
  task: FabricTask
  files: Awaited<ReturnType<typeof downloadFabricArtifact>>[]
  failedArtifacts: Array<{ id: string; error: string }>
}

/**
 * Confirm, submit, follow to the end, then download and checksum every
 * artifact. Aborting the tool call cancels the provider task.
 */
export async function runSubscriptionTask(ctx: Tool.Context, input: SubscriptionTaskInput): Promise<SubscriptionTaskResult> {
  const humanAction = await confirmSubscriptionTask(ctx, input)
  const task = await submitFabricTask(
    {
      capability: input.capability,
      prompt: input.prompt,
      routing: { provider: input.provider },
      options: input.options ?? {},
      priority: "interactive",
      idempotency_key: `gizzi-tool-${ctx.sessionID}-${ctx.callID ?? ctx.messageID}`,
    },
    humanAction,
    ctx.abort,
  )
  const onAbort = () => void cancelFabricTask(task.task_id)
  ctx.abort.addEventListener("abort", onAbort, { once: true })
  let final: FabricTask
  try {
    if (ctx.abort.aborted) onAbort()
    ctx.metadata({ title: `${input.title} — ${providerName(input.provider)} is working`, metadata: { taskID: task.task_id } })
    final = await followFabricTask(task.task_id, ctx.abort, {
      onProgress: (label) =>
        ctx.metadata({ title: `${input.title} — ${label}`, metadata: { taskID: task.task_id, progress: label } }),
    })
  } finally {
    ctx.abort.removeEventListener("abort", onAbort)
  }
  const files: SubscriptionTaskResult["files"] = []
  const failedArtifacts: SubscriptionTaskResult["failedArtifacts"] = []
  for (const id of final.result?.artifact_ids ?? []) {
    try {
      files.push(await downloadFabricArtifact(id, ctx.abort))
    } catch (error) {
      failedArtifacts.push({ id, error: error instanceof Error ? error.message : String(error) })
    }
  }
  return { task: final, files, failedArtifacts }
}

export function attachmentsFor(files: SubscriptionTaskResult["files"], title: string) {
  return files.map(
    (file, i): Omit<MessageV2.FilePart, "id" | "sessionID" | "messageID"> => ({
      type: "file",
      mime: file.mime,
      url: dataUrl(file),
      // The gateway names files by artifact id; name them after the request.
      filename: `${title}${files.length > 1 ? ` ${i + 1}` : ""}${extension(file)}`,
    }),
  )
}

function extension(file: { filename: string; mime: string }): string {
  const dot = file.filename.lastIndexOf(".")
  if (dot > 0) return file.filename.slice(dot)
  const byMime: Record<string, string> = {
    "image/png": ".png",
    "image/jpeg": ".jpg",
    "image/webp": ".webp",
    "application/pdf": ".pdf",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation": ".pptx",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document": ".docx",
    "text/markdown": ".md",
    "text/html": ".html",
  }
  return byMime[file.mime] ?? ""
}

/** Plain-language account of how a task ended (Register 1). */
export function describeOutcome(result: SubscriptionTaskResult, provider: string, noun: string): { ok: boolean; text: string } {
  const name = providerName(provider)
  const { task, files, failedArtifacts } = result
  const lines: string[] = []
  if (task.status === "needs_user") {
    const question = task.status_detail ?? task.error?.user_action ?? "it needs input on your Sessions computer"
    return {
      ok: false,
      text: `${name} paused this ${noun} and is asking: ${question}. The user answers provider questions themselves; Allternit does not answer them. Tell the user, and do not try to answer it for them.`,
    }
  }
  if (task.status === "cancelled") return { ok: false, text: `The ${noun} was cancelled before ${name} finished.` }
  if (task.status === "failed") {
    const detail = task.error?.detail ?? task.status_detail
    return { ok: false, text: `${name} could not make the ${noun}${detail ? `: ${detail}` : ""}.` }
  }
  if (task.status === "partial") lines.push(`${name} finished part of the ${noun}; some output did not come back.`)
  if (task.result?.text?.trim()) lines.push(task.result.text.trim())
  if (files.length) {
    lines.push(
      `Files (in the session's Outputs, checksum verified): ${files.map((f) => `${f.filename} (${f.mime}, ${f.bytes.byteLength} bytes)`).join("; ")}.`,
    )
  }
  for (const failed of failedArtifacts) lines.push(`Not returned: ${failed.error}`)
  if (!lines.length) lines.push(`${name} finished but returned no text or files.`)
  return { ok: task.status === "completed" && failedArtifacts.length === 0, text: lines.join("\n\n") }
}

// ── Capability tools ─────────────────────────────────────────────────────────

interface CapabilityToolSpec {
  id: string
  capability: string
  noun: string
  verb: string
  description: string
}

export const CAPABILITY_TOOLS: CapabilityToolSpec[] = [
  {
    id: "subscription_presentation_create",
    capability: "presentation.create",
    noun: "presentation",
    verb: "Create a presentation",
    description:
      "Create a slide deck (PPTX) with one of the user's connected subscriptions (e.g. Claude Slides, Kimi Slides), run on their Sessions computer. The user confirms every call in the app before anything is sent. Can take several minutes.",
  },
  {
    id: "subscription_document_create",
    capability: "document.create",
    noun: "document",
    verb: "Create a document",
    description:
      "Write a document (DOCX/PDF/Markdown) with one of the user's connected subscriptions, run on their Sessions computer. The user confirms every call in the app before anything is sent.",
  },
  {
    id: "subscription_research_deep",
    capability: "research.deep",
    noun: "research report",
    verb: "Run deep research",
    description:
      "Run a deep research task (a cited report, often 10–25 minutes) with one of the user's connected subscriptions, run on their Sessions computer. The user confirms every call in the app before anything is sent. If the provider asks a clarifying question, the user answers it — relay it, never answer it yourself.",
  },
]

interface CapabilityMeta {
  ok: boolean
  capability: string
  provider: string
  taskID?: string
  status?: string
  artifacts?: Array<{ id: string; filename: string; mime: string; sha256: string }>
}

function capabilityParams(providers: string[]) {
  return z.object({
    prompt: z.string().min(1).describe("What to make, in the user's words plus any detail they gave"),
    title: z.string().optional().describe("Short name for the output"),
    provider: z
      .string()
      .optional()
      .describe(`Which subscription to use: ${providers.join(", ")} (default ${providers[0]})`),
  })
}

function capabilityTool(spec: CapabilityToolSpec, providers: string[]): Tool.Info {
  return Tool.define<ReturnType<typeof capabilityParams>, CapabilityMeta>(spec.id, {
    description: `${spec.description}\n\nAvailable through: ${providers.map(providerName).join(", ")}.`,
    parameters: capabilityParams(providers),
    async execute(params, ctx) {
      const provider = params.provider ?? providers[0]
      const title = (params.title ?? params.prompt).trim().slice(0, 80) || spec.noun
      const live = availableProviders(await fabricCapabilities(), spec.capability)
      if (!live.includes(provider)) {
        return {
          title: `${spec.verb}: not available`,
          output: live.length
            ? `${providerName(provider)} cannot do this right now. Available: ${live.map(providerName).join(", ")}.`
            : `No connected subscription can do this right now. Nothing was sent.`,
          metadata: { ok: false, capability: spec.capability, provider },
        }
      }
      const result = await runSubscriptionTask(ctx, {
        capability: spec.capability,
        provider,
        prompt: params.prompt,
        title,
        summary: `${spec.verb} with your ${providerName(provider)} subscription: "${title}"`,
      })
      const outcome = describeOutcome(result, provider, spec.noun)
      return {
        title: `${spec.verb}: ${title}${outcome.ok ? "" : " — not finished"}`,
        output: outcome.text,
        metadata: {
          ok: outcome.ok,
          capability: spec.capability,
          provider,
          taskID: result.task.task_id,
          status: result.task.status,
          artifacts: result.files.map((f) => ({ id: f.artifactID, filename: f.filename, mime: f.mime, sha256: f.sha256 })),
        },
        attachments: attachmentsFor(result.files, title),
      }
    },
  })
}

/**
 * The capability tools the user's subscriptions can run right now (per the
 * gateway's GET /v1/capabilities). Empty when the fabric is not set up.
 */
export async function subscriptionCapabilityTools(): Promise<Tool.Info[]> {
  if (!fabricConfigured()) return []
  const entries = await fabricCapabilities()
  const tools: Tool.Info[] = []
  for (const spec of CAPABILITY_TOOLS) {
    const providers = availableProviders(entries, spec.capability)
    if (providers.length) tools.push(capabilityTool(spec, providers))
  }
  return tools
}
