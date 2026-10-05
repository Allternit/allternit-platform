/**
 * Agent Email client — external (mailflare-backed) email for platform agents.
 *
 * Shared between the CLI (`gizzi orchestration mail send-external` / `gizzi orchestration mail email-status`,
 * via the `@/cli/agent-email-client` re-export) and the runtime tools
 * (`send_agent_email` / `get_agent_email_status` in
 * `@/runtime/tools/builtins/agent-email`).
 *
 * Talks to the protected `/api/v1/agent-email/*` routes
 * (`cmd/allternit-api/src/agent_email_routes.rs`). Same base-URL + no-auth-header
 * pattern as the old Rails Mail client: `cmd/allternit-api/src/auth.rs:823-838`
 * trusts localhost-origin requests when `local_dev_bypass()`/`self_hosted()` is
 * enabled, which is the common case for a CLI user running against their own
 * local allternit-api instance. A remote/authenticated instance is out of scope.
 */

import { ALLTERNIT_GATEWAY_BASE } from "@/shared/constants/allternitGateway"

const API_BASE = ALLTERNIT_GATEWAY_BASE

/** Shape of `GET /api/v1/agent-email/status`. Fields beyond `configured` are
 * present only when the mailflare rail is configured. */
export interface AgentEmailStatus {
  configured: boolean
  domain?: string
  baseUrl?: string
  webhookSecretSet?: boolean
  reachable?: boolean
}

export interface SendAgentEmailInput {
  agentId: string
  to: string
  subject: string
  text?: string
  html?: string
}

/** Shape of `POST /api/v1/agent-email/send`. With mailflare's
 * REQUIRE_SEND_APPROVAL on (the default) the send is approval-gated and the
 * response carries the `mail:email-out-<uuid>` review thread; otherwise the
 * mail went straight to the provider queue. */
export interface SendAgentEmailResult {
  status: "pending_approval" | "sent"
  id: string
  thread?: string
  jobId?: string
  messageId?: string
}

async function agentEmailFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE}/api/v1${path}`, {
    ...init,
    headers: { "Content-Type": "application/json" },
  })
  if (!response.ok) {
    const text = await response.text().catch(() => "")
    throw new Error(`agent-email ${path} failed: ${response.status} ${response.statusText}${text ? ` — ${text}` : ""}`)
  }
  return (await response.json()) as T
}

/** `GET /api/v1/agent-email/status` — operator diagnostics for the mailflare rail. */
export async function getAgentEmailStatus(): Promise<AgentEmailStatus> {
  return agentEmailFetch<AgentEmailStatus>("/agent-email/status")
}

/** `POST /api/v1/agent-email/send` body `{ agent_id, to, subject, text?, html? }`. */
export async function sendAgentEmail(input: SendAgentEmailInput): Promise<SendAgentEmailResult> {
  return agentEmailFetch<SendAgentEmailResult>("/agent-email/send", {
    method: "POST",
    body: JSON.stringify({
      agent_id: input.agentId,
      to: input.to,
      subject: input.subject,
      text: input.text,
      html: input.html,
    }),
  })
}
