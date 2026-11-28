/**
 * Vault Write Tool
 *
 * Writes a note to the knowledge vault.
 */

import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { VaultManager } from "@/vault"
import { looksLikeSecret } from "@/shared/util/secret-redactor"

const DESCRIPTION = `Write or update a note in the Allternit Knowledge Vault.

Use this tool to persist important information that should be remembered across sessions:
- Decisions and their rationale
- Project updates and status changes
- Meeting summaries and action items
- Research findings
- Contact details or relationship notes

Notes are stored as Obsidian-compatible Markdown and can be browsed outside the agent.`

export const VaultWriteTool = Tool.define("vault_write", {
  description: DESCRIPTION,
  parameters: z.object({
    folder: z
      .enum(["Daily", "People", "Projects", "Meetings", "Topics", "Tasks"])
      .describe("Which folder to place the note in."),
    title: z.string().describe("Note title. Will be sanitized for use as a filename."),
    content: z.string().describe("Markdown body of the note."),
    tags: z.array(z.string()).optional().describe("Optional tags to attach."),
    entities: z.array(z.string()).optional().describe("Optional entity names mentioned (people, projects, companies)."),
    append_to_daily: z
      .boolean()
      .default(false)
      .describe("If true, appends to today's daily note instead of creating a standalone note."),
  }),
  async execute(params) {
    // Notes are plain Markdown that vault_query hands back to the model: a credential saved here
    // would reach model context. Credentials go to the secrets vault, which the kernel attaches
    // by handle (AllternitOS secret.use).
    if (looksLikeSecret(params.content) || looksLikeSecret(params.title)) {
      throw new Error(
        "This note holds what looks like a credential (an API key, token, password or private key). Notes are plain text the model reads back, so it was not saved. Store the credential in Allternit's credential vault and save a note that names it instead.",
      )
    }
    const vault = new VaultManager()
    await vault.initialize()

    let relPath: string
    let note: any

    if (params.append_to_daily) {
      const daily = await vault.getDailyNote()
      const section = [`\n## ${params.title}`, "", params.content, ""].join("\n")
      const updated = daily.content + section
      note = await vault.writeNote(daily.relPath, updated, {
        ...daily.frontmatter,
        tags: [...new Set([...(daily.frontmatter.tags || []), ...(params.tags || [])])],
      })
      relPath = daily.relPath
    } else {
      note = await vault.createNote(params.folder, params.title, params.content, {
        tags: params.tags,
        entities: params.entities,
      })
      relPath = note.relPath
    }

    vault.close()

    return {
      title: `Vault note written: ${params.title}`,
      output: `Saved note "${params.title}" to vault at \`${relPath}\`.`,
      metadata: { path: relPath, folder: params.folder, title: params.title },
    }
  },
})
