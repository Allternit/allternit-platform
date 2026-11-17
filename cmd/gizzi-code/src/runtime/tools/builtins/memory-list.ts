/**
 * memory_list (Primitives Wave D, D1 recall): the Memory Drive's topics with
 * counts, or one topic's entries `{id, version, text, added}`, 200 per page.
 * Every client; permission allow (loop/agent.ts). Both views say how many
 * entries need a look (conflict_of siblings from a concurrent edit).
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { MemoryDrive, type ListedEntry } from "@/runtime/memory/drive/drive"
import { needsLookLine } from "@/runtime/memory/drive/format"
import DESCRIPTION from "@/runtime/tools/builtins/memory-list.txt"

export const MEMORY_LIST_PAGE_SIZE = 200

interface ListMetadata {
  count: number
  ids: string[]
  ref: string
  topic?: string
  next?: string
  needsLook: number
}

function entryLine(e: ListedEntry): string {
  const flag = e.conflictOf ? `; needs a look: second version of ${e.conflictOf}` : ""
  return `- ${e.text} (id: ${e.id}; version: ${e.version}; added: ${e.added}${flag})`
}

export const MemoryListTool = Tool.define("memory_list", {
  description: DESCRIPTION,
  parameters: z.object({
    drive: z.string().optional().describe("A mounted drive's name or ref; leave it out for the personal drive"),
    topic: z.string().optional().describe("A topic file; leave it out to list topics with counts"),
    after: z.string().optional().describe("The last entry id of the previous page"),
  }),
  async execute(params, _ctx) {
    const ref = await MemoryDrive.resolveRef(params.drive)
    if (!params.topic?.trim()) {
      const listing = await MemoryDrive.listTopics(ref)
      const total = listing.topics.reduce((n, t) => n + t.count, 0)
      const lines = [`Memory Drive ${ref}: ${listing.topics.length} topic${listing.topics.length === 1 ? "" : "s"}, ${total} memor${total === 1 ? "y" : "ies"}.`]
      if (listing.needsLook.length > 0) lines.push(`${needsLookLine(listing.needsLook.length)}.`)
      lines.push("", ...listing.topics.map((t) => `- ${t.path.replace(/\.md$/, "")}: ${t.count}${t.needsLook ? ` (${t.needsLook} need a look)` : ""}`))
      if (listing.topics.length === 0) lines.push("(nothing saved yet)")
      if (listing.needsLook.length > 0) {
        lines.push("", "Need a look (keep the right text on the original id, delete the copy):", ...listing.needsLook.slice(0, 50).map(entryLine))
      }
      return {
        title: `Memory topics (${listing.topics.length})`,
        output: lines.join("\n"),
        metadata: { count: total, ids: listing.needsLook.map((e) => e.id), ref, needsLook: listing.needsLook.length } as ListMetadata,
      }
    }
    const page = await MemoryDrive.listEntries(ref, params.topic, { after: params.after, limit: MEMORY_LIST_PAGE_SIZE })
    const flagged = page.entries.filter((e) => e.conflictOf).length
    const lines = [`${ref}:${page.path}: ${page.total} memor${page.total === 1 ? "y" : "ies"}${page.entries.length < page.total ? `, showing ${page.entries.length}` : ""}.`]
    if (flagged > 0) lines.push(`${needsLookLine(flagged)} on this page.`)
    lines.push("", ...page.entries.map(entryLine))
    if (page.entries.length === 0) lines.push("(no entries)")
    if (page.next) lines.push("", `More: call memory_list again with after "${page.next}".`)
    return {
      title: `Memories in ${page.path} (${page.entries.length})`,
      output: lines.join("\n"),
      metadata: { count: page.entries.length, ids: page.entries.map((e) => e.id), ref, topic: page.path, next: page.next, needsLook: flagged } as ListMetadata,
    }
  },
})
