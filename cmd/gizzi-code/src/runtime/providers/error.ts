import { APICallError } from "ai"
import { STATUS_CODES } from "http"
import { iife } from "@/shared/util/iife"
import { Log } from "@/shared/util/log"
import { cloudCreditsRefusal, describeProviderError, extractProviderErrorCode } from "@/shared/util/provider-error"

const log = Log.create({ service: "provider.error" })

export namespace ProviderError {
  // Adapted from overflow detection patterns in:
  // https://github.com/badlogic/pi-mono/blob/main/packages/ai/src/utils/overflow.ts
  const OVERFLOW_PATTERNS = [
    /prompt is too long/i, // Anthropic
    /input is too long for requested model/i, // Amazon Bedrock
    /exceeds the context window/i, // OpenAI (Completions + Responses API message text)
    /input token count.*exceeds the maximum/i, // Google (Gemini)
    /maximum prompt length is \d+/i, // xAI (Grok)
    /reduce the length of the messages/i, // Groq
    /maximum context length is \d+ tokens/i, // OpenRouter, DeepSeek
    /exceeds the available context size/i, // llama.cpp server
    /greater than the context length/i, // LM Studio
    /context window exceeds limit/i, // MiniMax
    /exceeded model token limit/i, // Kimi For Coding, Moonshot
    /context[_ ]length[_ ]exceeded/i, // Generic fallback
  ]

  function isOpenAiErrorRetryable(e: APICallError) {
    const status = e.statusCode
    if (!status) return e.isRetryable
    // openai sometimes returns 404 for models that are actually available
    return status === 404 || e.isRetryable
  }

  // Providers not reliably handled in this function:
  // - z.ai: can accept overflow silently (needs token-count/context-window checks)
  function isOverflow(message: string) {
    if (OVERFLOW_PATTERNS.some((p) => p.test(message))) return true

    // Providers/status patterns handled outside of regex list:
    // - Cerebras: often returns "400 (no body)" / "413 (no body)"
    // - Mistral: often returns "400 (no body)" / "413 (no body)"
    return /^4(00|13)\s*(status code)?\s*\(no body\)/i.test(message)
  }

  function error(providerID: string, error: APICallError) {
    return error.message
  }

  function message(providerID: string, e: APICallError) {
    return iife(() => {
      const msg = e.message
      if (msg === "") {
        if (e.responseBody) return e.responseBody
        if (e.statusCode) {
          const err = STATUS_CODES[e.statusCode]
          if (err) return err
        }
        return "Unknown error"
      }

      const transformed = error(providerID, e)
      if (transformed !== msg) {
        return transformed
      }
      if (!e.responseBody || (e.statusCode && msg !== STATUS_CODES[e.statusCode])) {
        return msg
      }

      try {
        const body = JSON.parse(e.responseBody)
        // try to extract common error message fields
        const errMsg = body.message || body.error || body.error?.message
        if (errMsg && typeof errMsg === "string") {
          return `${msg}: ${errMsg}`
        }
      } catch (e) {
        log.debug("Failed to parse error response body", { error: e })
      }

      return `${msg}: ${e.responseBody}`
    }).trim()
  }

  function json(input: unknown) {
    if (typeof input === "string") {
      try {
        const result = JSON.parse(input)
        if (result && typeof result === "object") return result
        return undefined
      } catch {
        return undefined
      }
    }
    if (typeof input === "object" && input !== null) {
      return input
    }
    return undefined
  }

  export type ParsedStreamError =
    | {
        type: "context_overflow"
        message: string
        responseBody: string
      }
    | {
        type: "api_error"
        message: string
        isRetryable: false
        responseBody: string
      }

  export function parseStreamError(input: unknown): ParsedStreamError | undefined {
    const body = json(input)
    if (!body) return

    const responseBody = JSON.stringify(body)
    if (body.type !== "error") return

    switch (body?.error?.code) {
      case "context_length_exceeded":
        return {
          type: "context_overflow",
          message: "Input exceeds context window of this model",
          responseBody,
        }
      case "insufficient_quota":
        return {
          type: "api_error",
          message: "Quota exceeded. Check your plan and billing details.",
          isRetryable: false,
          responseBody,
        }
      case "usage_not_included":
        return {
          type: "api_error",
          message: "To use Codex with your ChatGPT plan, upgrade to Plus: https://chatgpt.com/explore/plus.",
          isRetryable: false,
          responseBody,
        }
      case "invalid_prompt":
        return {
          type: "api_error",
          message: typeof body?.error?.message === "string" ? body?.error?.message : "Invalid prompt.",
          isRetryable: false,
          responseBody,
        }
    }
  }

  export type ParsedAPICallError =
    | {
        type: "context_overflow"
        message: string
        responseBody?: string
      }
    | {
        type: "api_error"
        message: string
        statusCode?: number
        isRetryable: boolean
        responseHeaders?: Record<string, string>
        responseBody?: string
        metadata?: Record<string, string>
      }

  export type ParsedUnknownProviderError = {
    type: "api_error"
    message: string
    isRetryable: boolean
    statusCode?: number
    responseBody?: string
    metadata: Record<string, string>
  }

  /**
   * Normalizes provider failures that were not emitted as an AI SDK
   * APICallError. Subprocess adapters and several streaming providers throw
   * plain Error instances or provider-shaped JSON values; allowing those to
   * fall through as `Unknown` discards the provider identity and error class
   * before the CLI receives the session.error event.
   */
  export function parseUnknownProviderError(input: {
    providerID: string
    error: unknown
  }): ParsedUnknownProviderError {
    const source = input.error instanceof Error ? input.error.message : input.error
    const described = describeProviderError({ raw: source, providerID: input.providerID })
    const object = json(input.error) as Record<string, unknown> | undefined
    const statusCode =
      typeof object?.statusCode === "number"
        ? object.statusCode
        : typeof object?.status === "number"
          ? object.status
          : undefined
    const responseBody =
      typeof object?.responseBody === "string"
        ? object.responseBody
        : typeof object?.body === "string"
          ? object.body
          : undefined
    const originalName = input.error instanceof Error ? input.error.name : undefined
    // A Cloud credits refusal keeps its own code (credits_required /
    // credits_exhausted) so clients can open the credits disclosure.
    const credits = cloudCreditsRefusal(responseBody ?? source)
    const metadata: Record<string, string> = {
      providerID: input.providerID,
      code: credits?.code ?? described.code,
    }
    if (originalName) metadata.originalName = originalName
    // A person-plan failure (PersonPlanError) keeps the provider's own code and
    // reset time so the chat stream's error frame can carry them.
    const planCode = (input.error as { providerCode?: unknown } | undefined)?.providerCode
    const planReset = (input.error as { resetAt?: unknown } | undefined)?.resetAt
    if (typeof planCode === "string" && planCode && !credits) metadata.code = planCode
    if (typeof planReset === "string" && planReset) metadata.resetAt = planReset

    return {
      type: "api_error",
      message: credits?.message || described.message || `Provider "${input.providerID}" request failed`,
      isRetryable: described.code === "rate_limit" || described.code === "overloaded",
      statusCode,
      responseBody,
      metadata,
    }
  }

  export function parseAPICallError(input: { providerID: string; error: APICallError }): ParsedAPICallError {
    const m = message(input.providerID, input.error)
    if (isOverflow(m)) {
      return {
        type: "context_overflow",
        message: m,
        responseBody: input.error.responseBody,
      }
    }

    // Keep the body's `error.code` on the session error. Allternit Cloud's
    // 402 names `credits_required` / `credits_exhausted` there, and the web
    // adapter and the TUI open the credits disclosure from that code; before
    // 2026-11-15 only the url survived and the code was lost on the relay path.
    const metadata: Record<string, string> = {}
    if (input.error.url) metadata.url = input.error.url
    const credits = cloudCreditsRefusal(input.error.responseBody)
    const code = credits?.code ?? extractProviderErrorCode(input.error.responseBody)
    if (code) metadata.code = code
    return {
      type: "api_error",
      message: credits?.message ?? m,
      statusCode: input.error.statusCode,
      isRetryable: credits
        ? false
        : input.providerID.startsWith("openai")
          ? isOpenAiErrorRetryable(input.error)
          : input.error.isRetryable,
      responseHeaders: input.error.responseHeaders,
      responseBody: input.error.responseBody,
      metadata: Object.keys(metadata).length ? metadata : undefined,
    }
  }
}
