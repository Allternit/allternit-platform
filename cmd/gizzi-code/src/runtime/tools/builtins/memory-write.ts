import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Instance } from "@/runtime/context/project/instance"
import { MemoryDrive } from "@/runtime/memory/drive/drive"

const DESCRIPTION = `Save, update or delete a fact in the user's Memory Drive.

The Memory Drive is a git repo of markdown topic files. Each memory is ONE line
(a single durable fact); this tool records where it came from (this session)
and today's date, gives it a stable id, regenerates MEMORY.md's index and syncs.

Use it when:
- The user asks you to remember something ("remember X" → save immediately)
- You learn a stable preference, correction, or important project fact
- A saved memory is wrong or outdated (pass its id to update, or action "delete")

Do NOT save: session-specific details or in-progress work state, things
derivable from the code, unverified guesses, transcripts, logs, or anything
that looks like a credential (it will be refused).

To find an existing memory's id, use memory_recall first.`

export const MemoryWriteTool = Tool.define("memory_write", {
  description: DESCRIPTION,
  parameters: z.object({
    action: z.enum(["save", "delete"]).default("save").describe("save (create or update) or delete"),
    text: z
      .string()
      .optional()
      .describe("The fact, as one plain sentence (required for save). Lead with the rule/fact; include the why briefly if useful."),
    type: z
      .enum(["user", "feedback", "project", "reference"])
      .optional()
      .describe(
        "'user' = who the user is / preferences, 'feedback' = corrections or guidance, 'project' = ongoing work context, 'reference' = pointers to external resources",
      ),
    topic: z
      .string()
      .optional()
      .describe("Optional topic file (e.g. 'preferences', 'people', 'projects/gizzi'). Defaults by type."),
    id: z.string().optional().describe("Stable id of an existing memory to update or delete (from memory_recall)."),
    drive: z
      .string()
      .optional()
      .describe("Drive ref to write to (default 'personal'; e.g. 'project:<id>' for a mounted, writable drive)."),
  }),
  async execute(params, ctx) {
    const ref = params.drive?.trim() || "personal"
    if (params.action === "delete") {
      if (!params.id) throw new Error("Pass the id of the memory to delete (find it with memory_recall).")
      const { found, result } = await MemoryDrive.forget(params.id, { ref, sessionId: ctx.sessionID })
      if (!found) {
        return { title: "Memory not found", output: `No memory with id ${params.id} in ${ref}.`, metadata: { id: params.id, path: "", ref, pending: false } }
      }
      return {
        title: "Memory deleted",
        output: `Deleted ${params.id} from ${ref}.${syncNote(result)}`,
        metadata: { id: params.id, path: "", ref, pending: !!result?.pending },
      }
    }
    if (!params.text?.trim()) throw new Error("Pass the fact to remember as text.")
    const saved = await MemoryDrive.remember({
      text: params.text,
      type: params.type,
      topic: params.topic,
      id: params.id,
      ref,
      sessionId: ctx.sessionID,
      projectDir: Instance.directory,
    })
    return {
      title: saved.result.changed ? `Memory saved: ${saved.path}` : "Memory already saved",
      output: `Saved to ${saved.ref}:${saved.path} (id: ${saved.entry.id}).${syncNote(saved.result)}`,
      metadata: { id: saved.entry.id, path: saved.path, ref: saved.ref, pending: saved.result.pending },
    }
  },
})

function syncNote(result?: { pending: boolean; error?: string }): string {
  if (!result?.pending) return ""
  return ` Saved locally; not synced yet${result.error ? ` (${result.error})` : ""}. gizzi retries next session.`
}
