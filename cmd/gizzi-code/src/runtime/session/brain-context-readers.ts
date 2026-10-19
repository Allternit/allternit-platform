import os from "node:os"
import path from "node:path"
import { readdir, readFile } from "node:fs/promises"
import { ContextBreakdown } from "@/runtime/session/context-breakdown"
import { KimiContext } from "@/runtime/session/kimi-context"

/**
 * Brains whose own session record holds the full context (prompt, tools,
 * memory files, skills, messages). gizzi reads it after each turn, so the
 * panel shows the brain's real split rather than what gizzi sent it.
 * (Claude Code and Qwen report through their own `/context`, Codex through
 * its rollout — see the CLI driver.)
 */
export namespace BrainContextReaders {
  export interface ReadInput {
    sessionID: string
    providerID: string
    modelID: string
    vendorSessionId: string
    used?: number
    size: number
  }

  type Reader = (input: ReadInput) => Promise<ContextBreakdown.Snapshot | undefined>

  /** Kimi Code: `<KIMI_CODE_HOME>/sessions/<workspace>/<session>/agents/main/wire.jsonl`. */
  const kimi: Reader = async (input) => {
    const root = path.join(process.env.KIMI_CODE_HOME ?? path.join(os.homedir(), ".kimi-code"), "sessions")
    const id = input.vendorSessionId.startsWith("session_") ? input.vendorSessionId : `session_${input.vendorSessionId}`
    for (const workspace of await readdir(root).catch(() => [] as string[])) {
      const file = path.join(root, workspace, id, "agents", "main", "wire.jsonl")
      const wire = await readFile(file, "utf8").catch(() => undefined)
      if (wire === undefined) continue
      const agentFiles: Record<string, string> = {}
      for (const p of KimiContext.agentPaths(wire)) {
        const text = await readFile(p, "utf8").catch(() => "")
        if (text) agentFiles[p] = text
      }
      return KimiContext.build({ wire, agentFiles, ...input })
    }
    return undefined
  }

  const READERS: Record<string, Reader> = { "kimi-cli": kimi }

  export function has(providerID: string) {
    return providerID in READERS
  }

  export async function read(input: ReadInput) {
    return READERS[input.providerID]?.(input).catch(() => undefined)
  }
}
