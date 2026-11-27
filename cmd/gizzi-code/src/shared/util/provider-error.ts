import stripAnsi from "strip-ansi"

export type ProviderErrorKind = "rate_limit" | "insufficient_balance" | "overloaded" | "auth" | "generic"
export type ProviderErrorCode =
  | "rate_limit"
  | "insufficient_balance"
  | "unsupported_model"
  | "context_overflow"
  | "auth"
  | "overloaded"
  | "generic"

const CONTROL_CHARS = /[\u0000-\u0008\u000B-\u001F\u007F]/g
const RETRY_LIMIT = /\(retry limit reached after \d+ attempts?\)/i

const RATE_LIMIT_PATTERNS = [
  /too many requests/i,
  /rate limit/i,
  /retry[- ]after/i,
  /requests per/i,
  /request limit reached/i,
  /429\b/i,
]

const BALANCE_PATTERNS = [
  /insufficient balance/i,
  /insufficient quota/i,
  /quota exceeded/i,
  /free usage exceeded/i,
  /recharge/i,
  /billing/i,
  /credits?/i,
  /usage_not_included/i,
]

const OVERLOADED_PATTERNS = [/overloaded/i, /resource_exhausted/i, /temporarily unavailable/i, /service unavailable/i]

const AUTH_PATTERNS = [
  /unauthorized/i,
  /forbidden/i,
  /reauthenticate/i,
  /invalid api key/i,
  /invalid auth/i,
  /\b401\b/i,
  /\b403\b/i,
]

const UNSUPPORTED_MODEL_PATTERNS = [
  /model.*not supported/i,
  /unsupported model/i,
  /model not found/i,
  /unknown model/i,
]

const CONTEXT_OVERFLOW_PATTERNS = [/context[_ ]length[_ ]exceeded/i, /context window/i, /prompt is too long/i]

export function stripControl(text: string): string {
  return stripAnsi(text).replace(CONTROL_CHARS, " ")
}

export function extractProviderErrorMessage(raw: unknown): string {
  const seed = normalizeRaw(raw)
  if (!seed) return ""
  const extracted = extractJsonMessage(seed) ?? seed
  return normalizeWhitespace(extracted)
}

function normalizeRaw(raw: unknown): string {
  if (raw == null) return ""
  if (typeof raw === "string") return normalizeWhitespace(stripControl(raw))
  if (typeof raw === "number" || typeof raw === "bigint" || typeof raw === "boolean") return String(raw)
  if (typeof raw === "object") {
    try {
      return normalizeWhitespace(stripControl(JSON.stringify(raw)))
    } catch {
      return ""
    }
  }
  return ""
}

function normalizeWhitespace(value: string): string {
  return value.replace(/\r?\n+/g, " ").replace(/\s+/g, " ").trim()
}

function extractJsonMessage(value: string): string | undefined {
  const direct = parseJsonObject(value)
  if (direct) return extractMessageFromObject(direct)

  const embedded = parseEmbeddedJsonObject(value)
  if (!embedded) return undefined
  return extractMessageFromObject(embedded)
}

function parseJsonObject(candidate: string): Record<string, unknown> | undefined {
  try {
    const parsed = JSON.parse(candidate)
    if (!parsed || typeof parsed !== "object") return undefined
    return parsed as Record<string, unknown>
  } catch {
    return undefined
  }
}

function parseEmbeddedJsonObject(value: string): Record<string, unknown> | undefined {
  const start = value.indexOf("{")
  const end = value.lastIndexOf("}")
  if (start === -1 || end === -1 || end <= start) return undefined
  return parseJsonObject(value.slice(start, end + 1))
}

function extractMessageFromObject(parsed: Record<string, unknown>): string | undefined {
  const direct =
    readString(parsed, ["error", "message"]) ??
    readString(parsed, ["message"]) ??
    readString(parsed, ["detail"]) ??
    readString(parsed, ["error", "detail"]) ??
    readString(parsed, ["data", "message"])
  if (direct) return direct

  const nestedCandidates = [
    readString(parsed, ["responseBody"]),
    readString(parsed, ["error", "responseBody"]),
    readString(parsed, ["body"]),
    readString(parsed, ["error", "body"]),
    readString(parsed, ["data"]),
    readString(parsed, ["error", "data"]),
  ].filter(Boolean) as string[]

  for (const candidate of nestedCandidates) {
    const nested = extractMessageFromPotentialJsonString(candidate)
    if (nested) return nested
  }

  const rawError = parsed["error"]
  if (typeof rawError === "string" && rawError.trim()) return rawError
  return undefined
}

function extractMessageFromPotentialJsonString(candidate: string): string | undefined {
  if (!candidate) return undefined
  const normalized = normalizeWhitespace(stripControl(candidate))
  if (!normalized) return undefined
  const parsed = parseJsonObject(normalized) ?? parseEmbeddedJsonObject(normalized)
  if (!parsed) return undefined
  return extractMessageFromObject(parsed)
}

function readString(input: any, path: string[]): string | undefined {
  let current = input
  for (const segment of path) {
    if (!current || typeof current !== "object") return undefined
    current = current[segment]
  }
  return typeof current === "string" ? current : undefined
}

export function classifyProviderErrorKind(message: string): ProviderErrorKind {
  if (!message) return "generic"
  const value = message.toLowerCase()
  if (UNSUPPORTED_MODEL_PATTERNS.some((pattern) => pattern.test(value))) return "generic"
  if (CONTEXT_OVERFLOW_PATTERNS.some((pattern) => pattern.test(value))) return "generic"
  if (RATE_LIMIT_PATTERNS.some((pattern) => pattern.test(value))) return "rate_limit"
  if (BALANCE_PATTERNS.some((pattern) => pattern.test(value))) return "insufficient_balance"
  if (OVERLOADED_PATTERNS.some((pattern) => pattern.test(value))) return "overloaded"
  if (AUTH_PATTERNS.some((pattern) => pattern.test(value))) return "auth"
  return "generic"
}

export function providerErrorLabel(kind: ProviderErrorKind): string {
  if (kind === "rate_limit") return "Rate limited"
  if (kind === "insufficient_balance") return "Provider credits required"
  if (kind === "overloaded") return "Provider overloaded"
  if (kind === "auth") return "Provider authentication required"
  return "Provider error"
}

export function extractProviderErrorCode(raw: unknown): string | undefined {
  const source = normalizeRaw(raw)
  if (!source) return undefined
  const parsed = parseJsonObject(source) ?? parseEmbeddedJsonObject(source)
  if (!parsed) return undefined
  const code =
    readString(parsed, ["error", "code"]) ??
    readString(parsed, ["code"]) ??
    readString(parsed, ["error", "type"]) ??
    readString(parsed, ["type"])
  return code?.trim() || undefined
}

export function normalizeProviderErrorCode(code: string | undefined, message: string): ProviderErrorCode {
  const value = (code ?? "").toLowerCase()
  const detail = message.toLowerCase()
  if (
    value.includes("1302") ||
    value.includes("too_many_requests") ||
    value.includes("rate_limit") ||
    /\b429\b/.test(value)
  ) {
    return "rate_limit"
  }
  if (
    value.includes("1113") ||
    value.includes("insufficient_quota") ||
    value.includes("insufficient_balance") ||
    value.includes("usage_not_included") ||
    value.includes("billing")
  ) {
    return "insufficient_balance"
  }
  if (
    value.includes("model_not_supported") ||
    value.includes("unsupported_model") ||
    value.includes("model_not_found") ||
    UNSUPPORTED_MODEL_PATTERNS.some((pattern) => pattern.test(detail))
  ) {
    return "unsupported_model"
  }
  if (value.includes("context_length_exceeded") || CONTEXT_OVERFLOW_PATTERNS.some((pattern) => pattern.test(detail))) {
    return "context_overflow"
  }
  if (
    value.includes("invalid_api_key") ||
    value.includes("unauthorized") ||
    value.includes("forbidden") ||
    AUTH_PATTERNS.some((pattern) => pattern.test(detail))
  ) {
    return "auth"
  }
  if (value.includes("resource_exhausted") || value.includes("overloaded") || OVERLOADED_PATTERNS.some((pattern) => pattern.test(detail))) {
    return "overloaded"
  }
  const fallback = classifyProviderErrorKind(detail)
  if (fallback === "rate_limit") return "rate_limit"
  if (fallback === "insufficient_balance") return "insufficient_balance"
  if (fallback === "overloaded") return "overloaded"
  if (fallback === "auth") return "auth"
  return "generic"
}

export function providerErrorHint(input: {
  code: ProviderErrorCode
  providerID?: string
  message: string
}): string | undefined {
  if (input.code === "rate_limit") return "Wait for reset window or switch provider/model."
  if (input.code === "insufficient_balance") return "Add provider credits or use a free/connected model."
  if (input.code === "unsupported_model") {
    if (/codex-spark/i.test(input.message)) {
      return "Use GPT-5.2 Codex or switch to MiniMax/GLM on GIZZI.IO ZEN."
    }
    return "Select a model supported by this provider/account."
  }
  if (input.code === "context_overflow") return "Compact context or shorten the prompt."
  if (input.code === "auth") return "Re-authenticate this provider and retry."
  if (input.code === "overloaded") return "Provider is overloaded. Retry shortly or switch provider."
  return undefined
}

export function providerErrorTitle(code: ProviderErrorCode, kind: ProviderErrorKind): string {
  if (code === "rate_limit") return "Rate limited"
  if (code === "insufficient_balance") return "Provider credits required"
  if (code === "unsupported_model") return "Unsupported model"
  if (code === "context_overflow") return "Context window exceeded"
  if (code === "auth") return "Provider authentication required"
  if (code === "overloaded") return "Provider overloaded"
  return providerErrorLabel(kind)
}

export function describeProviderError(input: {
  raw: unknown
  providerID?: string
}): {
  message: string
  kind: ProviderErrorKind
  code: ProviderErrorCode
  title: string
  hint?: string
} {
  const message = extractProviderErrorMessage(input.raw)
  const kind = classifyProviderErrorKind(message)
  const code = normalizeProviderErrorCode(extractProviderErrorCode(input.raw), message)
  const title = providerErrorTitle(code, kind)
  const hint = providerErrorHint({
    code,
    providerID: input.providerID,
    message,
  })
  return { message, kind, code, title, hint }
}

export function isRetryLimitReached(message: string): boolean {
  return RETRY_LIMIT.test(message)
}

export function retryLimitNote(attempt: number): string {
  return `retry limit reached after ${attempt} attempt${attempt === 1 ? "" : "s"}`
}

export function formatRetryStatus(input: {
  message: string
  attempt: number
  next: number
  now: number
}): {
  label: string
  detail: string
} {
  const info = describeProviderError({ raw: input.message })
  const label = info.title
  const remaining = Math.max(0, Math.ceil((input.next - input.now) / 1000))
  const reason = info.message || label
  return {
    label: normalizeWhitespace(label),
    detail: `${reason} | retry in ${remaining}s | attempt ${input.attempt}`,
  }
}

/**
 * Allternit Cloud's "needs credits" refusal (HTTP 402, `error.code`
 * `credits_required` when the account has no credits row, `credits_exhausted`
 * when its balance is at or below 0). Matched by code, never by message
 * text, so every surface (TUI, `gizzi models`, the session error the web
 * adapter reads) shows the same disclosure.
 */
export type CloudCreditsCode = "credits_required" | "credits_exhausted"

export const CLOUD_BILLING_URL = "https://ai.allternit.com/settings/billing"
export const CLOUD_PACK_LABELS = ["$10", "$25", "$50", "$100"] as const

export type CloudCreditsRefusal = {
  code: CloudCreditsCode
  /** The server's own sentence, when the body carried one. */
  message?: string
  /** Pack labels from the body (`$10`…), when it carried them. */
  packs?: string[]
  billingUrl?: string
}

function isCloudCreditsCode(value: unknown): value is CloudCreditsCode {
  return value === "credits_required" || value === "credits_exhausted"
}

/**
 * The credits refusal in an error body, an SDK error message that embeds the
 * body ("402 {\"error\":{...}}"), or an already-parsed object; undefined for
 * anything else.
 */
export function cloudCreditsRefusal(raw: unknown): CloudCreditsRefusal | undefined {
  const source = normalizeRaw(raw)
  if (!source) return undefined
  const parsed = parseJsonObject(source) ?? parseEmbeddedJsonObject(source)
  const body = (parsed?.error && typeof parsed.error === "object" ? parsed.error : parsed) as Record<string, unknown> | undefined
  const code = body?.code
  if (!isCloudCreditsCode(code)) {
    // A truncated or wrapped body still names its code.
    const match = source.match(/"code"\s*:\s*"(credits_required|credits_exhausted)"/)
    return match ? { code: match[1] as CloudCreditsCode } : undefined
  }
  const packs = Array.isArray(body?.packs)
    ? body!.packs.flatMap((p: unknown) => {
        const label = (p as { label?: unknown } | null)?.label
        return typeof label === "string" && label.trim() ? [label.trim()] : []
      })
    : []
  const message = typeof body?.message === "string" && body.message.trim() ? body.message.trim() : undefined
  const billingUrl = typeof body?.billing_url === "string" && /^https:\/\//.test(body.billing_url) ? body.billing_url : undefined
  return { code, message, packs: packs.length ? packs : undefined, billingUrl }
}

/**
 * The one-line disclosure: "Allternit Cloud models need credits. Packs: $10,
 * $25, $50, $100. Buy: https://ai.allternit.com/settings/billing · or choose a plan."
 */
export function cloudCreditsDisclosure(
  code?: CloudCreditsCode | null,
  opts: { packs?: string[]; billingUrl?: string } = {},
): string {
  const lead = code === "credits_exhausted" ? "Your Allternit Cloud credits are used up." : "Allternit Cloud models need credits."
  const packs = (opts.packs?.length ? opts.packs : CLOUD_PACK_LABELS).join(", ")
  return `${lead} Packs: ${packs}. Buy: ${opts.billingUrl ?? CLOUD_BILLING_URL} · or choose a plan.`
}

/**
 * Allternit Open refusals (contract CONTRACT-open-tier-2026-11-26 §4):
 * 402 `open_plan_required`, 402 `open_lane_requires_credits` (reason
 * `allowance_used` or `bought_credits_required`), 403 `open_research_required`.
 * The relayed path can lose `error.code`, so a bare body is also matched by
 * status plus the body's own wording or `reason`.
 */
export type CloudOpenCode = "open_plan_required" | "open_lane_requires_credits" | "open_research_required"
export type CloudOpenReason = "allowance_used" | "bought_credits_required"

export type CloudOpenRefusal = { code: CloudOpenCode; reason?: CloudOpenReason; billingUrl?: string }

const OPEN_CODES: CloudOpenCode[] = ["open_plan_required", "open_lane_requires_credits", "open_research_required"]

export function cloudOpenRefusal(raw: unknown, status?: number): CloudOpenRefusal | undefined {
  const source = normalizeRaw(raw)
  if (!source) return undefined
  const parsed = parseJsonObject(source) ?? parseEmbeddedJsonObject(source)
  const body = (parsed?.error && typeof parsed.error === "object" ? parsed.error : parsed) as Record<string, unknown> | undefined
  let code = OPEN_CODES.find((c) => c === body?.code) ?? OPEN_CODES.find((c) => source.includes(`"${c}"`))
  const message = typeof body?.message === "string" ? body.message : source
  if (!code) {
    // Code lost on the relay: status + body wording.
    if (status === 403 && /open research/i.test(message)) code = "open_research_required"
    else if (status === 402 && /open models?.{0,40}(need|require).{0,20}paid plan|on free, run open models/i.test(message)) code = "open_plan_required"
    else if (status === 402 && /open-model allowance|open models run on credits you buy|open_lane/i.test(message)) code = "open_lane_requires_credits"
  }
  if (!code) return undefined
  let reason: CloudOpenReason | undefined
  if (code === "open_lane_requires_credits") {
    const r = body?.reason
    reason =
      r === "allowance_used" || r === "bought_credits_required"
        ? r
        : /bought_credits_required|credits you buy/i.test(source)
          ? "bought_credits_required"
          : "allowance_used"
  }
  const url = body?.billing_url ?? body?.plans_url
  const billingUrl = typeof url === "string" && /^https:\/\//.test(url) ? url : undefined
  return { code, reason, billingUrl }
}

/** One line per refusal: contract §4 copy plus the billing URL. */
export function cloudOpenDisclosure(refusal: CloudOpenRefusal): string {
  const url = refusal.billingUrl ?? CLOUD_BILLING_URL
  switch (refusal.code) {
    case "open_plan_required":
      return `Open models on Allternit Cloud need a paid plan. On Free, run open models on your own device with Freeweights. Plans: ${url}`
    case "open_research_required":
      return "This model is part of Open Research, available by agreement."
    default:
      return refusal.reason === "bought_credits_required"
        ? `On Plus, open models run on credits you buy. Add credits to use them: ${url}`
        : `This month's open-model allowance is used. Add credits to keep going: ${url}`
  }
}
