/**
 * inbox: the model's tool for the user's inbox (AllternitOS Stage 1, step 14).
 * Contract: handoffs/allternit-os-2026-11-27/CONTRACT.md sections 7.2, 11.
 *
 * Exactly four actions: list, summarise, snooze, prepare. There is NO approve,
 * deny or resolve action and no path to one: a person approves in /inbox, in
 * `gizzi inbox approve`, or in any surface's inbox. The kernel refuses a model
 * caller anyway (section 4.3). Talks to `/api/v1/inbox` through the shared
 * allternitApi client (runtime/services/api/inbox.ts).
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { getInboxItems, inboxLine, listInbox, prepareInbox, snoozeInbox, type InboxItem } from "@/runtime/services/api/inbox"
import DESCRIPTION from "@/runtime/tools/builtins/inbox.txt"

/** Copied from spec/Contracts/os/v1/schemas/inbox.schema.json, InboxAction and InboxToolInputV1 (1.0.0). */
export const InboxAction = z.enum(["list", "summarise", "snooze", "prepare"])
export type InboxAction = z.infer<typeof InboxAction>

const Params = z.object({
  action: InboxAction,
  scope: z.enum(["needs_you", "snoozed", "all"]).optional().describe("list: which items (default needs_you)"),
  limit: z.number().int().min(1).optional().describe("list: most items to return"),
  ids: z.array(z.string().min(1)).optional().describe("summarise: item ids"),
  id: z.string().min(1).optional().describe("snooze, prepare: the item id"),
  until: z.string().min(1).optional().describe("snooze: ISO timestamp"),
  draft: z.record(z.string(), z.unknown()).optional().describe("prepare: the draft answer"),
})
type Input = z.infer<typeof Params>

function fail(error: string, message: string) {
  return { title: `Inbox: ${error}`, output: JSON.stringify({ ok: false, error, message }), metadata: { ok: false, error } }
}

function summary(items: InboxItem[]): string {
  if (!items.length) return "Nothing there."
  return items
    .map((i) => `- ${i.action_summary} (${i.action_class}, ${i.urgency}, due ${i.deadline}, ${i.kind === "approval" ? "needs a person's decision" : "for your information"})`)
    .join("\n")
}

export const InboxTool = Tool.define("inbox", {
  description: DESCRIPTION,
  parameters: Params,
  async execute(params: Input) {
    try {
      switch (params.action) {
        case "list": {
          const items = await listInbox(params.scope ?? "needs_you", params.limit)
          return { title: `Inbox: ${items.length} item${items.length === 1 ? "" : "s"}`, output: JSON.stringify(items), metadata: { ok: true, action: "list", count: items.length } }
        }
        case "summarise": {
          if (!params.ids?.length) return fail("bad_request", "summarise needs ids.")
          const { found, missing } = await getInboxItems(params.ids)
          const text = summary(found) + (missing.length ? `\nNot found: ${missing.join(", ")}.` : "")
          return { title: "Inbox: summary", output: text, metadata: { ok: true, action: "summarise", count: found.length } }
        }
        case "snooze": {
          if (!params.id) return fail("bad_request", "snooze needs id.")
          if (!params.until) return fail("bad_request", "snooze needs until.")
          await snoozeInbox(params.id, params.until)
          return { title: "Inbox: snoozed", output: JSON.stringify({ ok: true }), metadata: { ok: true, action: "snooze", id: params.id } }
        }
        case "prepare": {
          if (!params.id) return fail("bad_request", "prepare needs id.")
          if (!params.draft) return fail("bad_request", "prepare needs draft.")
          // The result is an InboxPrepareResult: the item with state "prepared" and a draft
          // (action_class, action_text, payload_hash). A prepared item still needs a person's approve.
          const result = await prepareInbox(params.id, params.draft)
          return {
            title: "Inbox: draft prepared",
            output: JSON.stringify(result ?? { ok: true }),
            metadata: { ok: true, action: "prepare", id: params.id, state: "prepared" },
          }
        }
        default:
          return fail("bad_request", "Unknown action. The inbox tool can list, summarise, snooze and prepare. It cannot approve or deny.")
      }
    } catch (e) {
      return fail("failed", e instanceof Error ? e.message : String(e))
    }
  },
})
