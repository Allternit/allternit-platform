import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { getAgentEmailStatus, sendAgentEmail } from "@/runtime/services/api/agentEmail"
import { getRuntimeAgentId } from "@/shared/utils/agentHardBans"

const SEND_DESCRIPTION = `Send an external email from this agent's provisioned Allternit Mail address (mailflare rail).

Approval gate: sends are held for human approval by default — the tool does NOT deliver
the email directly. It returns a review thread id (mail:email-out-<uuid>) that a human
approves or rejects in the Factory mail review queue (CLI: gizzi orchestration mail decide <threadId> --approve).
Only when the operator has disabled the approval requirement does the send go straight to
the provider queue (status "sent").

Sender identity: the agent_id selects which provisioned agent address sends the mail.
Defaults to this runtime's ALLTERNIT_AGENT_ID when launched as a platform bot session;
otherwise pass agent_id explicitly.

Use this only for genuine external correspondence the user asked for. This tool is covered
by dispatch-time hard bans: agents whose character card bans email_send /
external_communication are blocked before this runs.`

export const SendAgentEmailTool = Tool.define("send_agent_email", {
  description: SEND_DESCRIPTION,
  parameters: z.object({
    agent_id: z
      .string()
      .optional()
      .describe("Platform agent id with a provisioned email channel. Defaults to ALLTERNIT_AGENT_ID."),
    to: z.string().describe("Recipient email address"),
    subject: z.string().describe("Subject line"),
    text: z.string().optional().describe("Plain-text body"),
    html: z.string().optional().describe("HTML body"),
  }),
  async execute(params) {
    const agentId = params.agent_id ?? getRuntimeAgentId()
    if (!agentId) {
      throw new Error(
        "agent_id is required: this runtime has no ALLTERNIT_AGENT_ID set, so pass agent_id explicitly.",
      )
    }
    if (!params.text && !params.html) {
      throw new Error("one of text or html is required")
    }
    const result = await sendAgentEmail({
      agentId,
      to: params.to,
      subject: params.subject,
      text: params.text,
      html: params.html,
    })
    const metadata: {
      status: "pending_approval" | "sent"
      id: string
      thread?: string
      messageId?: string
      jobId?: string
    } = {
      status: result.status,
      id: result.id,
      thread: result.thread,
      messageId: result.messageId,
      jobId: result.jobId,
    }
    if (result.status === "pending_approval") {
      return {
        title: "Email held for approval",
        output: `Email queued for human approval — review thread: ${result.thread ?? result.id}. It will not be delivered until a human approves that thread.`,
        metadata,
      }
    }
    return {
      title: "Email sent",
      output: `Email sent (message id: ${result.messageId ?? result.id}).`,
      metadata,
    }
  },
})

const STATUS_DESCRIPTION = `Show the status of the Allternit Mail (agent email) rail: whether the
mailflare-backed external email channel is configured, its domain, and whether it is
reachable. Use this to diagnose send_agent_email failures.`

export const GetAgentEmailStatusTool = Tool.define("get_agent_email_status", {
  description: STATUS_DESCRIPTION,
  parameters: z.object({}),
  async execute() {
    const status = await getAgentEmailStatus()
    if (!status.configured) {
      return {
        title: "Agent email rail not configured",
        output:
          "Agent email rail: not configured (mailflare env unset on allternit-api; agent provisioning falls back to a Factory mint address).",
        metadata: { ...status },
      }
    }
    return {
      title: `Agent email rail: ${status.reachable ? "reachable" : "UNREACHABLE"}`,
      output: `Agent email rail: configured — domain ${status.domain ?? "?"} — ${status.reachable ? "reachable" : "UNREACHABLE"}`,
      metadata: { ...status },
    }
  },
})
