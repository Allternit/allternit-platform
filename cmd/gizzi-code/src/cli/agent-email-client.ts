/**
 * Agent Email client — external (mailflare-backed) email for platform agents,
 * used by `gizzi orchestration mail send-external` / `gizzi orchestration mail email-status`.
 *
 * The implementation moved to `@/runtime/services/api/agentEmail` so the
 * runtime tools (`send_agent_email` / `get_agent_email_status`) can share it
 * without runtime code importing from `src/cli/`. This module remains as the
 * stable CLI-facing import surface.
 */

export {
  getAgentEmailStatus,
  sendAgentEmail,
  type AgentEmailStatus,
  type SendAgentEmailInput,
  type SendAgentEmailResult,
} from "@/runtime/services/api/agentEmail"
