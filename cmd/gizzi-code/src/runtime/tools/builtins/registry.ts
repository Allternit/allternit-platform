import { QuestionTool } from "@/runtime/tools/builtins/question"
import { PaneBrowserTool } from "@/runtime/tools/builtins/pane-browser"
import { PaneArtifactTool } from "@/runtime/tools/builtins/pane-artifact"
import { MediaGenerateTool } from "@/runtime/tools/builtins/media-generate"
import { VerifyTool } from "@/runtime/tools/builtins/verify"
import { BashTool } from "@/runtime/tools/builtins/bash"
import { EditTool } from "@/runtime/tools/builtins/edit"
import { GlobTool } from "@/runtime/tools/builtins/glob"
import { GrepTool } from "@/runtime/tools/builtins/grep"
import { BatchTool } from "@/runtime/tools/builtins/batch"
import { ReadTool } from "@/runtime/tools/builtins/read"
import { TaskTool } from "@/runtime/tools/builtins/task"
import { TodoWriteTool } from "@/runtime/tools/builtins/todo"
import { WebFetchTool } from "@/runtime/tools/builtins/webfetch"
import { WriteTool } from "@/runtime/tools/builtins/write"
import { InvalidTool } from "@/runtime/tools/builtins/invalid"
import { SkillTool } from "@/runtime/tools/builtins/skill"
import { AgentCommunicateTool } from "@/runtime/tools/builtins/agent-communicate"
import { ListTool } from "@/runtime/tools/builtins/ls"
import { MultiEditTool } from "@/runtime/tools/builtins/multiedit"
import type { Agent } from "@/runtime/loop/agent"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Instance } from "@/runtime/context/project/instance"
import { Config } from "@/runtime/context/config/config"
import path from "path"
import { type ToolContext as PluginToolContext, type ToolDefinition } from "@allternit/plugin/tool"
import z from "zod/v4"
import { Plugin } from "@/runtime/integrations/plugin"
import { WebSearchTool } from "@/runtime/tools/builtins/websearch"
import { CodeSearchTool } from "@/runtime/tools/builtins/codesearch"
import { Flag } from "@/runtime/context/flag/flag"
import { Log } from "@/shared/util/log"
import { LspTool } from "@/runtime/tools/builtins/lsp"
import { BrowserToolsetTool, ComputerToolsetTool } from "@/runtime/tools/computer-toolset"
import { DesktopTool } from "@/runtime/tools/builtins/desktop"
import { Truncate } from "@/runtime/tools/builtins/truncation"
import { PlanExitTool, PlanEnterTool } from "@/runtime/tools/builtins/plan"
import { ApplyPatchTool } from "@/runtime/tools/builtins/apply_patch"
import { NotebookEditTool } from "@/runtime/tools/builtins/notebook"
import { MemoryWriteTool } from "@/runtime/tools/builtins/memory-write"
import { MemoryRecallTool } from "@/runtime/tools/builtins/memory-recall"
import { VaultQueryTool } from "@/runtime/tools/builtins/vault-query"
import { VaultWriteTool } from "@/runtime/tools/builtins/vault-write"
import { Glob } from "@/shared/util/glob"
import { CreateGoalTool, GetGoalTool, SetGoalBudgetTool, UpdateGoalTool } from "@/runtime/tools/builtins/goal"
import { AgentSwarmTool } from "@/runtime/tools/builtins/agent-swarm"
import {
  BackgroundTaskListTool,
  BackgroundTaskOutputTool,
  BackgroundTaskStopTool,
} from "@/runtime/tools/builtins/background-task"
import {
  SkillActivateTool,
  SkillDecideTool,
  SkillEvaluateTool,
  SkillProposeTool,
  SkillRollbackTool,
} from "@/runtime/tools/builtins/skill-growth"
import { SkillImportApplyTool, SkillImportPreviewTool } from "@/runtime/tools/builtins/skill-import"
import {
  ScratchpadListTool,
  ScratchpadReadTool,
  ScratchpadRemoveTool,
  ScratchpadWriteTool,
} from "@/runtime/tools/builtins/scratchpad"
import { GetAgentEmailStatusTool, SendAgentEmailTool } from "@/runtime/tools/builtins/agent-email"
import { MessageAgentTool } from "@/runtime/tools/builtins/message-agent"
import { MdxGraphTool } from "@/runtime/tools/builtins/mdx-graph"
import { subscriptionCapabilityTools } from "@/runtime/tools/builtins/subscription"

export namespace ToolRegistry {
  const log = Log.create({ service: "tool.registry" })

  export const state = Instance.state(async () => {
    const custom = [] as Tool.Info[]

    const matches = await Config.directories().then((dirs) =>
      dirs.flatMap((dir) =>
        Glob.scanSync("{tool,tools}/*.{js,ts}", { cwd: dir, absolute: true, dot: true, symlink: true }),
      ),
    )
    if (matches.length) await Config.waitForDependencies()
    for (const match of matches) {
      const namespace = path.basename(match, path.extname(match))
      const mod = await import(match)
      for (const [id, def] of Object.entries<ToolDefinition>(mod)) {
        custom.push(fromPlugin(id === "default" ? namespace : `${namespace}_${id}`, def))
      }
    }

    const plugins = await Plugin.list()
    for (const plugin of plugins) {
      for (const [id, def] of Object.entries(plugin.tool ?? {})) {
        custom.push(fromPlugin(id, def))
      }
    }

    return { custom }
  })

  function fromPlugin(id: string, def: ToolDefinition): Tool.Info {
    return {
      id,
      init: async (initCtx) => ({
        parameters: z.object(def.args),
        description: def.description,
        execute: async (args, ctx) => {
          const pluginCtx = {
            ...ctx,
            directory: Instance.workdir,
            worktree: Instance.worktree,
          } as unknown as PluginToolContext
          const result = await def.execute(args as any, pluginCtx)
          const out = await Truncate.output(result as string, {}, initCtx?.agent)
          return {
            title: "",
            output: out.truncated ? out.content : (result as string),
            metadata: { truncated: out.truncated, outputPath: out.truncated ? out.outputPath : undefined },
          }
        },
      }),
    }
  }

  export async function register(tool: Tool.Info) {
    const { custom } = await state()
    const idx = custom.findIndex((t) => t.id === tool.id)
    if (idx >= 0) {
      custom.splice(idx, 1, tool)
      return
    }
    custom.push(tool)
  }

  async function all(): Promise<Tool.Info[]> {
    const custom = await state().then((x) => x.custom)
    const config = await Config.get()
    const question = ["app", "cli", "desktop"].includes(Flag.GIZZI_CLIENT) || Flag.GIZZI_ENABLE_QUESTION_TOOL

    return [
      InvalidTool,
      ...(question ? [QuestionTool] : []),
      // Acts on the page shown in the app's browser pane; needs an app client.
      ...(question ? [PaneBrowserTool] : []),
      // Works on the document open in the app's artifact pane; needs an app client.
      ...(question ? [PaneArtifactTool] : []),
      // Images (native lane renders in the app); needs an app client.
      ...(question ? [MediaGenerateTool] : []),
      AgentCommunicateTool,
      VerifyTool,
      BashTool,
      ReadTool,
      GlobTool,
      GrepTool,
      ListTool,
      EditTool,
      MultiEditTool,
      WriteTool,
      TaskTool,
      AgentSwarmTool,
      BackgroundTaskListTool,
      BackgroundTaskOutputTool,
      BackgroundTaskStopTool,
      ...(Flag.GIZZI_DISABLE_SCRATCHPAD
        ? []
        : [ScratchpadListTool, ScratchpadReadTool, ScratchpadWriteTool, ScratchpadRemoveTool]),
      WebFetchTool,
      TodoWriteTool,
      CreateGoalTool,
      GetGoalTool,
      SetGoalBudgetTool,
      UpdateGoalTool,
      WebSearchTool,
      CodeSearchTool,
      SkillTool,
      SkillProposeTool,
      SkillEvaluateTool,
      SkillDecideTool,
      SkillActivateTool,
      SkillRollbackTool,
      SkillImportPreviewTool,
      SkillImportApplyTool,
      ApplyPatchTool,
      NotebookEditTool,
      MdxGraphTool,
      MemoryWriteTool,
      MemoryRecallTool,
      VaultQueryTool,
      VaultWriteTool,
      SendAgentEmailTool,
      GetAgentEmailStatusTool,
      // Bot Mode (B4/D5): listed unconditionally — the registry has no session
      // context. SessionPrompt.resolveTools deletes it from the per-session
      // tool record unless the session is a canonical bot chat.
      MessageAgentTool,
      ...(Flag.GIZZI_ENABLE_LSP_TOOL ? [LspTool] : []),
      ...(config.experimental?.batch_tool === true ? [BatchTool] : []),
      ...(Flag.GIZZI_CLIENT === "cli" ? [PlanExitTool, PlanEnterTool] : []),
      // The computer toolset (allternit.browser.v1 / allternit.computer.v1):
      // one executor in allternit-api for every model. `browser` replaces the
      // old ACU-gateway browser tool under the same flag; `computer` drives
      // GIZZI_COMPUTER_ID (default this-device) and is opt-in.
      ...(Flag.GIZZI_ENABLE_BROWSER_TOOL ? [BrowserToolsetTool] : []),
      ...(Flag.GIZZI_ENABLE_COMPUTER_TOOL ? [ComputerToolsetTool] : []),
      ...(Flag.GIZZI_ENABLE_DESKTOP_TOOL ? [DesktopTool] : []),
      // Subscription capabilities (presentations, documents, deep research)
      // the user's connected subscriptions can run now; every call is
      // confirmed by the user (D16). Empty when the fabric is not set up.
      ...(await subscriptionCapabilityTools()),
      ...custom,
    ]
  }

  export async function ids() {
    return all().then((x) => x.map((t) => t.id))
  }

  export async function tools(
    model: {
      providerID: string
      modelID: string
      npm?: string
    },
    agent?: Agent.Info,
  ) {
    const tools = await all()
    const result = await Promise.all(
      tools
        .filter((t) => {
          // use apply tool in same format as codex
          const usePatch =
            model.modelID.includes("gpt-") && !model.modelID.includes("oss") && !model.modelID.includes("gpt-4")
          if (t.id === "apply_patch") return usePatch
          if (t.id === "edit" || t.id === "write" || t.id === "multiedit") return !usePatch

          return true
        })
        .map(async (t) => {
          using _ = log.time(t.id)
          const tool = await t.init({ agent, model })
          const output = {
            description: tool.description,
            parameters: tool.parameters,
          }
          await Plugin.trigger("tool.definition", { toolID: t.id }, output)
          return {
            id: t.id,
            ...tool,
            description: output.description,
            parameters: output.parameters,
          }
        }),
    )
    return result
  }
}
