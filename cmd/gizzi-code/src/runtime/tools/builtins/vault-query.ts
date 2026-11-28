/**
 * Vault Query Tool
 *
 * Queries the knowledge vault for relevant context.
 */

import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { VaultManager } from "@/vault"
import { SecretRedactor } from "@/shared/util/secret-redactor"

const DESCRIPTION = `Query the Allternit Knowledge Vault for relevant notes, people, projects, meetings, and decisions.

The vault is a long-lived memory store built from your emails, calendar events, meeting transcripts,
and manually created notes. Use this tool when you need context about:
- Past conversations or decisions
- People you have interacted with
- Projects and their status
- Meeting history and action items
- Any topic that has been saved to the vault

Returns matching notes with titles, snippets, and file paths.`

export const VaultQueryTool = Tool.define("vault_query", {
  description: DESCRIPTION,
  parameters: z.object({
    query: z.string().describe("Search query text — can be a topic, person name, project name, or keyword."),
    entity_types: z
      .array(z.enum(["person", "project", "meeting", "decision", "topic", "daily", "task", "reference"]))
      .optional()
      .describe("Optional filter by entity types."),
    folder: z.string().optional().describe("Optional filter by folder (e.g., 'People', 'Projects', 'Meetings')."),
    limit: z.number().default(10).describe("Maximum number of results to return."),
    depth: z.number().default(1).describe("If 2+, traverses backlinks to find connected notes."),
  }),
  async execute(params) {
    const vault = new VaultManager()
    await vault.initialize()

    const result = await vault.query({
      text: params.query,
      entityTypes: params.entity_types as any,
      folder: params.folder,
      limit: params.limit,
    })

    // A secret saved in a note (or in a note written before vault_write refused them) never
    // reaches the model: previews are masked whole, before they are cut (AllternitOS step 10).
    const redactor = new SecretRedactor()
    const lines: string[] = [
      `Vault query: "${params.query}"`,
      `Found ${result.total} note(s)${result.truncated ? " (truncated)" : ""}`,
      "",
    ]

    for (const note of result.notes) {
      lines.push(`## ${redactor.text(note.title)}`)
      lines.push(`- **Folder:** ${note.folder || "root"}`)
      lines.push(`- **Tags:** ${note.frontmatter.tags?.join(", ") || "none"}`)
      lines.push(`- **Path:** ${note.relPath}`)

      if (params.depth >= 2) {
        const connected = await vault.traverse(note.relPath, 2)
        const connectedTitles = Array.from(connected.values())
          .filter(n => n.relPath !== note.relPath)
          .slice(0, 5)
          .map(n => `[[${redactor.text(n.title)}]]`)
        if (connectedTitles.length) {
          lines.push(`- **Connected:** ${connectedTitles.join(", ")}`)
        }
      }

      const masked = redactor.text(note.content)
      const preview = masked.slice(0, 300).replace(/\n/g, " ")
      lines.push(`- **Preview:** ${preview}${masked.length > 300 ? "..." : ""}`)
      lines.push("")
    }

    vault.close()

    return {
      title: `Vault query: ${params.query}`,
      output: lines.join("\n"),
      metadata: { total: result.total, truncated: result.truncated, count: result.notes.length },
    }
  },
})
