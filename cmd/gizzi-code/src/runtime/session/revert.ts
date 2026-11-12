import z from "zod/v4"
import { Identifier } from "@/shared/id/id"
import { Snapshot } from "@/runtime/session/snapshot"
import { MessageV2 } from "@/runtime/session/message-v2"
import { Session } from "@/runtime/session"
import { Log } from "@/shared/util/log"
import { randomUUID } from "node:crypto"
import { Database, desc, eq } from "@/runtime/session/storage/db"
import { MessageBranchTable, MessageTable, PartTable } from "@/runtime/session/session.sql"
import { Storage } from "@/runtime/session/storage/storage"
import { Bus } from "@/shared/bus"
import { SessionPrompt } from "@/runtime/session/prompt"
import { SessionSummary } from "@/runtime/session/summary"

export namespace SessionRevert {
  const log = Log.create({ service: "session.revert" })

  export const RevertInput = z.object({
    sessionID: Identifier.schema("session"),
    messageID: Identifier.schema("message"),
    partID: Identifier.schema("part").optional(),
  })
  export type RevertInput = z.infer<typeof RevertInput>

  export async function revert(input: RevertInput) {
    SessionPrompt.assertNotBusy(input.sessionID)
    const all = await Session.messages({ sessionID: input.sessionID })
    let lastUser: MessageV2.User | undefined
    const session = await Session.get(input.sessionID)

    let revert: Session.Info["revert"]
    const patches: Snapshot.Patch[] = []
    for (const msg of all) {
      if (msg.info.role === "user") lastUser = msg.info
      const remaining = []
      for (const part of msg.parts) {
        if (revert) {
          if (part.type === "patch") {
            patches.push(part)
          }
          continue
        }

        if (!revert) {
          if ((msg.info.id === input.messageID && !input.partID) || part.id === input.partID) {
            // if no useful parts left in message, same as reverting whole message
            const partID = remaining.some((item) => ["text", "tool"].includes(item.type)) ? input.partID : undefined
            revert = {
              messageID: !partID && lastUser ? lastUser.id : msg.info.id,
              partID,
            }
          }
          remaining.push(part)
        }
      }
    }

    if (revert) {
      const session = await Session.get(input.sessionID)
      revert.snapshot = session.revert?.snapshot ?? (await Snapshot.track())
      await Snapshot.revert(patches)
      if (revert.snapshot) revert.diff = await Snapshot.diff(revert.snapshot)
      const rangeMessages = all.filter((msg) => msg.info.id >= revert!.messageID)
      const diffs = await SessionSummary.computeDiff({ messages: rangeMessages })
      await Storage.write(["session_diff", input.sessionID], diffs)
      Bus.publish(Session.Event.Diff, {
        sessionID: input.sessionID,
        diff: diffs,
      })
      return Session.setRevert({
        sessionID: input.sessionID,
        revert,
        summary: {
          additions: diffs.reduce((sum, x) => sum + x.additions, 0),
          deletions: diffs.reduce((sum, x) => sum + x.deletions, 0),
          files: diffs.length,
        },
      })
    }
    return session
  }

  export async function unrevert(input: { sessionID: string }) {
    log.info("unreverting", input)
    SessionPrompt.assertNotBusy(input.sessionID)
    const session = await Session.get(input.sessionID)
    if (!session.revert) return session
    if (session.revert.snapshot) await Snapshot.restore(session.revert.snapshot)
    return Session.clearRevert(input.sessionID)
  }

  export async function cleanup(session: Session.Info) {
    if (!session.revert) return
    const sessionID = session.id
    const msgs = await Session.messages({ sessionID })
    const messageID = session.revert.messageID
    const preserve = [] as MessageV2.WithParts[]
    const remove = [] as MessageV2.WithParts[]
    let target: MessageV2.WithParts | undefined
    for (const msg of msgs) {
      if (msg.info.id < messageID) {
        preserve.push(msg)
        continue
      }
      if (msg.info.id > messageID) {
        remove.push(msg)
        continue
      }
      if (session.revert.partID) {
        preserve.push(msg)
        target = msg
        continue
      }
      remove.push(msg)
    }
    let removeParts: MessageV2.Part[] = []
    if (session.revert.partID && target) {
      const partID = session.revert.partID
      const removeStart = target.parts.findIndex((part) => part.id === partID)
      if (removeStart >= 0) {
        removeParts = target.parts.slice(removeStart)
        target.parts = target.parts.slice(0, removeStart)
      }
    }
    // Keep what is about to be deleted as one branch row (the earlier version
    // of this turn), in the same transaction as the deletes: either the old
    // version is kept and removed, or nothing changes.
    const removed: MessageV2.WithParts[] = []
    if (target && removeParts.length > 0) removed.push({ info: target.info, parts: removeParts })
    removed.push(...remove)
    Database.transaction((db) => {
      if (removed.length > 0) {
        db.insert(MessageBranchTable)
          .values({
            id: `brn_${randomUUID().replace(/-/g, "")}`,
            session_id: sessionID,
            anchor_message_id: messageID,
            created_at: Date.now(),
            payload: removed.map(branchMessage),
          })
          .run()
      }
      for (const msg of remove) db.delete(MessageTable).where(eq(MessageTable.id, msg.info.id)).run()
      for (const part of removeParts) db.delete(PartTable).where(eq(PartTable.id, part.id)).run()
    })
    for (const msg of remove) {
      await Bus.publish(MessageV2.Event.Removed, { sessionID: sessionID, messageID: msg.info.id })
    }
    for (const part of removeParts) {
      await Bus.publish(MessageV2.Event.PartRemoved, {
        sessionID: sessionID,
        messageID: target!.info.id,
        partID: part.id,
      })
    }
    await Session.clearRevert(sessionID)
  }

  /** Tool output kept per tool part in a branch row. */
  export const BRANCH_OUTPUT_LIMIT = 8 * 1024
  /** Newest branch rows `branches()` returns. */
  export const BRANCH_LIST_LIMIT = 20

  export type Branch = {
    id: string
    anchorMessageId: string
    createdAt: number
    messages: MessageV2.WithParts[]
  }

  function cap(text: string): string {
    if (Buffer.byteLength(text, "utf8") <= BRANCH_OUTPUT_LIMIT) return text
    return Buffer.from(text, "utf8").subarray(0, BRANCH_OUTPUT_LIMIT).toString("utf8") + "\n…[truncated]"
  }

  /** A removed message as stored in a branch row: tool outputs capped, inline file data dropped. */
  function branchMessage(msg: MessageV2.WithParts): MessageV2.WithParts {
    const parts = msg.parts.map((part): MessageV2.Part => {
      if (part.type === "tool") {
        const state = { ...part.state } as Record<string, any>
        if (typeof state.output === "string") state.output = cap(state.output)
        if (typeof state.error === "string") state.error = cap(state.error)
        if (state.metadata && typeof state.metadata.output === "string") {
          state.metadata = { ...state.metadata, output: cap(state.metadata.output) }
        }
        if (Array.isArray(state.attachments)) {
          state.attachments = state.attachments.map((file: MessageV2.FilePart) => stripInline(file))
        }
        return { ...part, state } as MessageV2.Part
      }
      if (part.type === "file") return stripInline(part)
      return part
    })
    return { info: msg.info, parts }
  }

  /** A data: URL (an inline attachment, up to 24 MB) is not copied into the branch row. */
  function stripInline(file: MessageV2.FilePart): MessageV2.FilePart {
    if (typeof file.url === "string" && file.url.startsWith("data:") && file.url.length > BRANCH_OUTPUT_LIMIT) {
      return { ...file, url: "" }
    }
    return file
  }

  /** Earlier versions of turns in this session, newest first (max 20). */
  export function branches(sessionID: string): Branch[] {
    const rows = Database.use((db) =>
      db
        .select()
        .from(MessageBranchTable)
        .where(eq(MessageBranchTable.session_id, sessionID))
        .orderBy(desc(MessageBranchTable.created_at), desc(MessageBranchTable.id))
        .limit(BRANCH_LIST_LIMIT)
        .all(),
    )
    return rows.map((row) => ({
      id: row.id,
      anchorMessageId: row.anchor_message_id,
      createdAt: row.created_at,
      messages: row.payload,
    }))
  }
}
