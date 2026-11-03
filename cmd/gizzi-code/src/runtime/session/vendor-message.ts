import { Session } from "./index"
import { MessageV2 } from "./message-v2"
import { Identifier } from "../../shared/id/id"
import { Instance } from "../context/project/instance"
import { selectAnswerPolicy, checkAnswerResponse, type AnswerPolicy } from "../integrations/artifacts/answer-policy"
import { SessionTrace } from "./trace"

/**
 * Append a vendor bot's reply (Agent Gateway) to a session as an assistant
 * message, without running a model turn. Deduped by `metadata.remote_event_id`
 * so a re-pulled vendor event never lands twice.
 */
export namespace VendorMessage {
  export async function append(input: {
    sessionID: string
    text: string
    metadata?: Record<string, unknown>
  }): Promise<{ id: string; duplicate: boolean }> {
    const remoteEventID = input.metadata?.["remote_event_id"]
    const all = await Session.messages({ sessionID: input.sessionID })
    if (typeof remoteEventID === "string" && remoteEventID !== "") {
      for (const m of all) {
        const hit = m.parts.find((p) => p.type === "text" && (p as any).metadata?.["remote_event_id"] === remoteEventID)
        if (hit) return { id: m.info.id, duplicate: true }
      }
    }
    const parent = [...all].reverse().find((m) => m.info.role === "user")
    const gatewayRecord = input.metadata?.answer_policy as { selected?: string; status?: string } | undefined
    const user = parent?.info.role === "user" ? parent.info : undefined
    const request = parent?.parts.filter((p): p is MessageV2.TextPart => p.type === "text" && !p.synthetic && !p.ignored).map(p => p.text).join("\n") ?? ""
    // A real stored request takes precedence over vendor metadata. Gateway
    // sessions that have no local user part carry the server's selected ID;
    // it is still checked against the full data contract here.
    const policy: AnswerPolicy | undefined = user
      ? selectAnswerPolicy(request, { visuals: !!user.system?.includes("## Enforced answer templates"), structuredOutput: user.format?.type === "json_schema" })
      : gatewayRecord?.selected ? { version: 1, mode: "template", template: gatewayRecord.selected as Extract<AnswerPolicy, {mode:"template"}>["template"], reason: "job" } : undefined
    const checked = policy ? checkAnswerResponse(policy, input.text) : { valid: true, problems: [] }
    const visible = checked.valid ? input.text : "This vendor answer could not be validated. Try again or choose Text only."
    const record = policy ? { version: 1 as const, selected: policy.mode === "template" ? policy.template : undefined, status: checked.valid ? "accepted" as const : "rejected" as const, attempt: 1, problems: checked.problems } : undefined
    const now = Date.now()
    const id = Identifier.ascending("message")
    await Session.updateMessage({
      id,
      sessionID: input.sessionID,
      role: "assistant",
      parentID: parent?.info.id ?? "",
      modelID: String(input.metadata?.["adapter"] ?? "vendor"),
      providerID: String(input.metadata?.["vendor"] ?? "vendor"),
      mode: "vendor",
      agent: String(input.metadata?.["vendor"] ?? "vendor"),
      path: { cwd: Instance.directory, root: Instance.worktree },
      cost: 0,
      tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
      finish: "stop",
      time: { created: now, completed: now },
      ...(record ? { answerPolicy: record } : {}),
    } as MessageV2.Assistant)
    await Session.updatePart({
      id: Identifier.ascending("part"),
      messageID: id,
      sessionID: input.sessionID,
      type: "text",
      text: visible,
      time: { start: now, end: now },
      metadata: { source: "vendor", ...(input.metadata ?? {}), ...(record ? { answerPolicy: record } : {}) },
    })
    if (record) SessionTrace.append({ sessionID: input.sessionID, messageID: id, kind: "answer.validation", data: record })
    await Session.touch(input.sessionID)
    return { id, duplicate: false }
  }
}
