import os from "node:os"
import path from "node:path"
import { readdir, readFile } from "node:fs/promises"
import { ContextBreakdown } from "@/runtime/session/context-breakdown"
import { KimiContext } from "@/runtime/session/kimi-context"
import { BrainRelay } from "@/runtime/session/brain-relay"

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

  /** MCP server names from OpenCode's config, so `<server>_<tool>` names count as MCP tools. */
  async function opencodeMcpServers(): Promise<string[]> {
    const dir = process.env.XDG_CONFIG_HOME ? path.join(process.env.XDG_CONFIG_HOME, "opencode") : path.join(os.homedir(), ".config", "opencode")
    const names = new Set<string>(["allternit"])
    for (const file of ["opencode.json", "opencode.jsonc", "config.json"]) {
      const text = await readFile(path.join(dir, file), "utf8").catch(() => "")
      if (!text) continue
      try {
        const config = JSON.parse(text.replace(/^\s*\/\/.*$/gm, ""))
        for (const name of Object.keys(config?.mcp ?? {})) names.add(name)
      } catch {}
    }
    return [...names]
  }

  /** CLIs whose provider calls pass through BrainRelay: the last request they sent. */
  const relayed: Reader = async (input) =>
    BrainRelay.breakdown({ ...input, mcpServers: input.providerID === "opencode" ? await opencodeMcpServers() : [] })

  const READERS: Record<string, Reader> = { "kimi-cli": kimi, opencode: relayed }

  export function has(providerID: string) {
    return providerID in READERS
  }

  export async function read(input: ReadInput) {
    return READERS[input.providerID]?.(input).catch(() => undefined)
  }
}
