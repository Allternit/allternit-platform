/**
 * memory_write: save, update, append to, replace part of, or delete one
 * Memory Drive entry. Changes to an existing entry carry `if_version` (the
 * version memory_list / memory_recall returned); a stale or missing version
 * is refused with `{refused, current, version}` so the model re-reads instead
 * of overwriting a newer edit. The secret gate runs on the resulting line.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Instance } from "@/runtime/context/project/instance"
import { EntryMissingError, MemoryDrive, ReplaceMatchError } from "@/runtime/memory/drive/drive"
import { EntryVersionError } from "@/runtime/memory/drive/format"
import { MemoryKernelAdapter } from "@/runtime/memory/kernel-adapter"
import DESCRIPTION from "@/runtime/tools/builtins/memory-write.txt"

interface WriteMetadata {
  id: string
  path: string
  ref: string
  pending: boolean
  version?: string
  refused?: boolean
  current?: string
  conflict?: string
}

export const MemoryWriteTool = Tool.define("memory_write", {
  description: DESCRIPTION,
  parameters: z.object({
    action: z
      .enum(["save", "append", "replace", "delete"])
      .default("save")
      .describe("save (create, or update by id), append (add text to the end of an entry), replace (swap part of an entry) or delete"),
    text: z
      .string()
      .optional()
      .describe(
        "save: the fact, as one plain sentence. append: the text to add, exactly as given (start it with a space to separate words).",
      ),
    old: z.string().optional().describe("replace: the exact text to change; it must appear exactly once in the entry."),
    new: z.string().optional().describe("replace: the text to put in its place (empty removes it)."),
    if_version: z
      .string()
      .optional()
      .describe("The entry's version from memory_list or memory_recall. Required to change or delete an existing entry."),
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
    id: z.string().optional().describe("Stable id of an existing entry (required for append, replace and delete)."),
    drive: z
      .string()
      .optional()
      .describe("Drive ref to write to (default 'personal'; e.g. 'project:<id>' for a mounted, writable drive)."),
  }),
  async execute(params, ctx) {
    const ref = params.drive?.trim() || "personal"
    const meta = (extra: Partial<WriteMetadata> = {}): WriteMetadata => ({ id: params.id ?? "", path: "", ref, pending: false, ...extra })
    try {
      if (params.action === "delete") {
        if (!params.id) throw new Error("Pass the id of the memory to delete (find it with memory_list or memory_recall).")
        const { found, result } = await MemoryDrive.forget(params.id, {
          ref,
          sessionId: ctx.sessionID,
          ifVersion: params.if_version,
          requireVersion: true,
        })
        if (!found) return { title: "Memory not found", output: `No memory with id ${params.id} in ${ref}.`, metadata: meta() }
        MemoryKernelAdapter.report("forgot", "Removed 1 memory", [{ text: params.id }], ctx.sessionID)
        return {
          title: "Memory deleted",
          output: `Deleted ${params.id} from ${ref}.${syncNote(result)}`,
          metadata: meta({ pending: !!result?.pending }),
        }
      }

      if (params.action === "append" || params.action === "replace") {
        if (!params.id) throw new Error(`Pass the id of the memory to ${params.action === "append" ? "add to" : "change"}.`)
        const edited =
          params.action === "append"
            ? await MemoryDrive.edit({
                action: "append",
                id: params.id,
                text: params.text ?? "",
                ifVersion: params.if_version,
                requireVersion: true,
                ref,
                sessionId: ctx.sessionID,
              })
            : await MemoryDrive.edit({
                action: "replace",
                id: params.id,
                old: params.old ?? "",
                new: params.new ?? "",
                ifVersion: params.if_version,
                requireVersion: true,
                ref,
                sessionId: ctx.sessionID,
              })
        if (edited.result.changed) {
          MemoryKernelAdapter.report("saved", `Edited: ${edited.entry.text}`, [{ text: edited.entry.text, source: edited.entry.source }], ctx.sessionID)
        }
        return {
          title: edited.conflict ? "Memory edit kept beside a newer one" : edited.result.changed ? "Memory edited" : "Memory unchanged",
          output: savedLine(edited.ref, edited.path, edited.entry.id, edited.version, edited.conflict) + syncNote(edited.result),
          metadata: meta({ path: edited.path, pending: edited.result.pending, version: edited.version, conflict: edited.conflict }),
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
        ifVersion: params.if_version,
        requireVersion: true,
      })
      if (saved.result.changed) {
        MemoryKernelAdapter.report("saved", `Saved: ${saved.entry.text}`, [{ text: saved.entry.text, source: saved.entry.source }], ctx.sessionID)
      }
      return {
        title: saved.conflict ? "Memory kept beside a newer one" : saved.result.changed ? `Memory saved: ${saved.path}` : "Memory already saved",
        output: savedLine(saved.ref, saved.path, saved.entry.id, saved.version, saved.conflict) + syncNote(saved.result),
        metadata: meta({ id: saved.entry.id, path: saved.path, ref: saved.ref, pending: saved.result.pending, version: saved.version, conflict: saved.conflict }),
      }
    } catch (error) {
      if (error instanceof EntryVersionError) {
        return {
          title: "Memory edit refused",
          output: [
            error.reason === "missing"
              ? `Refused: memory ${error.id} already exists, and changing it needs its version.`
              : `Refused: memory ${error.id} changed since you read it.`,
            `Current text: ${error.current}`,
            `Current version: ${error.version}`,
            "Check that your change still applies to the current text, then retry with this version as if_version.",
          ].join("\n"),
          metadata: meta({ id: error.id, refused: true, current: error.current, version: error.version }),
        }
      }
      if (error instanceof EntryMissingError || error instanceof ReplaceMatchError) {
        return { title: "Memory edit refused", output: error.message, metadata: meta({ refused: true }) }
      }
      throw error
    }
  },
})

function savedLine(ref: string, path: string, id: string, version?: string, conflict?: string): string {
  const base = `Saved to ${ref}:${path} (id: ${id}${version ? `; version: ${version}` : ""}).`
  if (!conflict) return base
  return `${base} Another device changed this memory at the same time, so its text stays under ${id} and yours was kept as ${conflict} (marked conflict_of). Tell the user which one to keep if it matters.`
}

function syncNote(result?: { pending: boolean; error?: string }): string {
  if (!result?.pending) return ""
  return ` Saved locally; not synced yet${result.error ? ` (${result.error})` : ""}. gizzi retries next session.`
}
