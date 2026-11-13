/**
 * Runs self-hosted tool calls with gizzi's own tools, on the worker.
 *
 * Each claimed session gets a local shadow session (gizzi's tools keep
 * per-session state: read-before-edit times, background tasks, scratchpad),
 * an explicit sandbox policy built from the claim's environment policy, and
 * the environment's git shim on PATH. Every call goes through
 * `ToolDispatcher.executeInitialized` inside `Instance.provide` rooted at the
 * workdir, so hard bans and hooks apply as they do locally, and `bash` runs
 * under `Sandbox.wrap` (bubblewrap on Linux, sandbox-exec on macOS) with that
 * policy. Linux without bubblewrap fails closed (bash.ts refuses).
 */
import os from "node:os"
import path from "node:path"
import { createHash } from "node:crypto"
import { statSync } from "node:fs"
import type { Tool } from "@/runtime/tools/builtins/tool"
import { ToolDispatcher } from "@/runtime/tools/dispatch"
import { RemoteToolHost } from "@/runtime/tools/remote-host"
import { Instance } from "@/runtime/context/project/instance"
import { SessionSandbox } from "@/runtime/context/sandbox/session-sandbox"
import { Sandbox } from "@/runtime/integrations/shell/sandbox"
import { EnvironmentRuntime } from "@/runtime/session/environment/runtime-state"
import { Truncate } from "@/runtime/tools/builtins/truncation"
import { ProcessRegistry } from "@/runtime/process-registry"
import { Identifier } from "@/shared/id/id"
import { Global } from "@/runtime/context/global"
import { Session } from "@/runtime/session"
import { BashTool } from "@/runtime/tools/builtins/bash"
import { ReadTool } from "@/runtime/tools/builtins/read"
import { WriteTool } from "@/runtime/tools/builtins/write"
import { EditTool } from "@/runtime/tools/builtins/edit"
import { MultiEditTool } from "@/runtime/tools/builtins/multiedit"
import { ApplyPatchTool } from "@/runtime/tools/builtins/apply_patch"
import { NotebookEditTool } from "@/runtime/tools/builtins/notebook"
import { GrepTool } from "@/runtime/tools/builtins/grep"
import { GlobTool } from "@/runtime/tools/builtins/glob"
import { ListTool } from "@/runtime/tools/builtins/ls"
import { SymbolEditTool } from "@/runtime/tools/builtins/symbol-edit"
import {
  BackgroundTaskListTool,
  BackgroundTaskOutputTool,
  BackgroundTaskStopTool,
} from "@/runtime/tools/builtins/background-task"
import {
  ScratchpadListTool,
  ScratchpadReadTool,
  ScratchpadRemoveTool,
  ScratchpadWriteTool,
} from "@/runtime/tools/builtins/scratchpad"
import { effectiveNetwork, installGitPolicy, sandboxPolicyFor, type EnvironmentPolicy } from "./egress"
import type { ToolCall } from "./client"
import type { DaemonLog } from "@/runtime/fabric-transport/daemon-util"

export interface ToolResult {
  output: string
  is_error: boolean
}

export interface PreparedSession {
  sessionId: string
  workId: string
  environmentId?: string
  policy: EnvironmentPolicy
}

/** What the worker loop needs from whatever runs the tools (tests inject a fake). */
export interface ToolExecutor {
  prepare(session: PreparedSession): Promise<void>
  run(sessionId: string, call: ToolCall, signal: AbortSignal): Promise<ToolResult>
  finish(sessionId: string): Promise<void>
}

const TOOLS: Tool.Info[] = [
  BashTool,
  ReadTool,
  WriteTool,
  EditTool,
  MultiEditTool,
  ApplyPatchTool,
  NotebookEditTool,
  GrepTool,
  GlobTool,
  ListTool,
  SymbolEditTool,
  BackgroundTaskListTool,
  BackgroundTaskOutputTool,
  BackgroundTaskStopTool,
  ScratchpadListTool,
  ScratchpadReadTool,
  ScratchpadWriteTool,
  ScratchpadRemoveTool,
]

/** Light instance init: what the sandbox tools need, none of the server-side startup (no LSP, sidecar, discovery). */
async function workerInit() {
  ProcessRegistry.install()
  Truncate.init()
}

function realGitPath(exclude: string): string | null {
  const PATH = process.env.PATH ?? ""
  for (const dir of PATH.split(path.delimiter)) {
    if (!dir || dir.startsWith(exclude)) continue
    const candidate = path.join(dir, "git")
    try {
      if (statSync(candidate).isFile()) return candidate
    } catch {
      // not here
    }
  }
  return null
}

export function createGizziToolExecutor(opts: { workdir: string; log: DaemonLog; stateDir?: string }): ToolExecutor {
  // This process runs tools; it must never forward them again.
  delete process.env.GIZZI_TOOL_HOST

  const workdir = path.resolve(opts.workdir)
  const stateDir = opts.stateDir ?? path.join(Global.Path.data, "selfhost-worker")
  const shadows = new Map<string, string>()
  const byId = new Map(TOOLS.map((tool) => [tool.id, tool]))
  const initialized = new Map<string, ReturnType<Tool.Info["init"]>>()

  const provide = <T>(fn: () => Promise<T>) => Instance.provide({ directory: workdir, init: workerInit, fn })

  function initTool(id: string) {
    let pending = initialized.get(id)
    if (!pending) {
      const tool = byId.get(id)
      if (!tool) return undefined
      pending = tool.init()
      initialized.set(id, pending)
    }
    return pending
  }

  return {
    async prepare(session) {
      await provide(async () => {
        let local = shadows.get(session.sessionId)
        if (!local) {
          const created = await Session.createNext({ directory: workdir, title: `self-hosted ${session.sessionId}` })
          local = created.id
          shadows.set(session.sessionId, local)
        }

        const key = createHash("sha256").update(session.sessionId).digest("hex").slice(0, 16)
        const git = await installGitPolicy({
          dir: path.join(stateDir, "git-policy", key),
          policy: session.policy,
          realGit: realGitPath(stateDir),
        })

        // Policy may differ from an earlier claim of the same session: stop the old proxy first.
        await Sandbox.cleanupProfile(local)
        SessionSandbox.enable(local, sandboxPolicyFor(session.policy, workdir, git?.readPaths ?? []))

        const env: Record<string, string> = {}
        if (git) env.PATH = `${git.binDir}${path.delimiter}${process.env.PATH ?? ""}`
        await EnvironmentRuntime.set(local, {
          environmentID: session.environmentId ?? "selfhost",
          network: effectiveNetwork(session.policy),
          env,
          workspace: workdir,
          dirs: [],
        })
        opts.log("info", "worker.session_prepared", {
          work_id: session.workId,
          session_id: session.sessionId,
          network: effectiveNetwork(session.policy),
          git_policy: Boolean(git),
          sandbox: Sandbox.detect(),
        })
      })
    },

    async run(sessionId, call, signal) {
      const local = shadows.get(sessionId)
      if (!local) return { output: "The worker has no prepared session for this call.", is_error: true }
      const name = RemoteToolHost.canonicalName(call.name)
      if (!byId.has(name)) {
        return {
          output: `The worker does not run the tool "${call.name}". Worker tools: ${[...byId.keys()].join(", ")}.`,
          is_error: true,
        }
      }
      return provide(async () => {
        // Tool init reads Instance (bash's description names the workdir), so it runs inside provide.
        let tool: Awaited<ReturnType<Tool.Info["init"]>>
        try {
          tool = await initTool(name)!
        } catch (error) {
          initialized.delete(name)
          return { output: `The worker could not load the ${name} tool: ${(error as Error).message}`, is_error: true }
        }
        const ctx: Tool.Context = {
          sessionID: local,
          messageID: Identifier.ascending("message"),
          agent: "build",
          abort: signal,
          callID: call.tool_call_id,
          messages: [],
          metadata() {},
          // Headless: the server decided to run this call; there is nobody to ask.
          async ask() {},
        }
        try {
          const result = await ToolDispatcher.executeInitialized(name, call.input ?? {}, ctx, tool.execute)
          return { output: String(result?.output ?? ""), is_error: false }
        } catch (error) {
          return { output: error instanceof Error ? error.message : String(error), is_error: true }
        }
      })
    },

    async finish(sessionId) {
      const local = shadows.get(sessionId)
      if (!local) return
      shadows.delete(sessionId)
      await provide(async () => {
        await Sandbox.cleanupProfile(local).catch(() => {})
        await EnvironmentRuntime.dispose(local).catch(() => {})
        await Session.remove(local).catch(() => {})
      }).catch((error) => opts.log("warn", "worker.session_cleanup_failed", { session_id: sessionId, error: String(error) }))
    },
  }
}

export const DEFAULT_WORKDIR = os.platform() === "win32" ? path.join(os.homedir(), "workspace") : "/workspace"
