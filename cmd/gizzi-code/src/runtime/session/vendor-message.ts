import { Session } from "./index"
import { MessageV2 } from "./message-v2"
import { Identifier } from "../../shared/id/id"
import { Instance } from "../context/project/instance"

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
    } as MessageV2.Assistant)
    await Session.updatePart({
      id: Identifier.ascending("part"),
      messageID: id,
      sessionID: input.sessionID,
      type: "text",
      text: input.text,
      time: { start: now, end: now },
      metadata: { source: "vendor", ...(input.metadata ?? {}) },
    })
    await Session.touch(input.sessionID)
    return { id, duplicate: false }
  }
}
