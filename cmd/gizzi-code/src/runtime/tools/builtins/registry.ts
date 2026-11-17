import { QuestionTool } from "@/runtime/tools/builtins/question"
import { PaneBrowserTool } from "@/runtime/tools/builtins/pane-browser"
import { CaptureScreenContextTool } from "@/runtime/tools/builtins/capture-screen-context"
import { BrowserTasksTool, browserTasksAvailableNow } from "@/runtime/tools/builtins/browser-tasks"
import { PaneArtifactTool } from "@/runtime/tools/builtins/pane-artifact"
import { MediaGenerateTool } from "@/runtime/tools/builtins/media-generate"
import { ArtifactCheckTool } from "@/runtime/tools/builtins/artifact-check"
import { ARTIFACT_TOOLS } from "@/runtime/tools/builtins/artifact"
import { VerifyTool } from "@/runtime/tools/builtins/verify"
import { BashTool } from "@/runtime/tools/builtins/bash"
import { EditTool } from "@/runtime/tools/builtins/edit"
import { GlobTool } from "@/runtime/tools/builtins/glob"
import { GrepTool } from "@/runtime/tools/builtins/grep"
import { BatchTool } from "@/runtime/tools/builtins/batch"
import { ReadTool } from "@/runtime/tools/builtins/read"
import { TaskTool } from "@/runtime/tools/builtins/task"
import { TodoWriteTool } from "@/runtime/tools/builtins/todo"
import { GlanceTool } from "@/runtime/tools/builtins/glance"
import { CurrentTimeTool } from "@/runtime/tools/builtins/current-time"
import { LookupTool } from "@/runtime/tools/builtins/lookup"
import { SendUserMessageTool } from "@/runtime/tools/builtins/send-user-message"
import { AutomationTool } from "@/runtime/tools/builtins/automation"
import { AskUserLaterTool } from "@/runtime/tools/builtins/ask-user-later"
import { WaitAgentsTool } from "@/runtime/tools/builtins/wait-agents"
import { AgentControlTool } from "@/runtime/tools/builtins/agent-control"
import { ToolSearchTool } from "@/runtime/tools/builtins/tool-search"
import { GetContextRemainingTool } from "@/runtime/tools/builtins/context-remaining"
import { HistoryRecallTool } from "@/runtime/tools/builtins/history-recall"
import { WebFetchTool } from "@/runtime/tools/builtins/webfetch"
import { WriteTool } from "@/runtime/tools/builtins/write"
import { InvalidTool } from "@/runtime/tools/builtins/invalid"
import { SkillTool } from "@/runtime/tools/builtins/skill"
import { AgentCommunicateTool } from "@/runtime/tools/builtins/agent-communicate"
import { ListTool } from "@/runtime/tools/builtins/ls"
import { MultiEditTool } from "@/runtime/tools/builtins/multiedit"
import { RunCodeTool } from "@/runtime/tools/builtins/run_code"
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
import { SymbolEditTool } from "@/runtime/tools/builtins/symbol-edit"
import { BrowserToolsetTool, ComputerToolsetTool, ComputerV2ToolsetTool } from "@/runtime/tools/computer-toolset"
import { ComputersTool } from "@/runtime/tools/builtins/computers"
import { Truncate } from "@/runtime/tools/builtins/truncation"
import { PlanExitTool, PlanEnterTool } from "@/runtime/tools/builtins/plan"
import { ApplyPatchTool } from "@/runtime/tools/builtins/apply_patch"
import { FolderAccessTool } from "@/runtime/tools/builtins/folder-access"
import { NotebookEditTool } from "@/runtime/tools/builtins/notebook"
import { MemoryWriteTool } from "@/runtime/tools/builtins/memory-write"
import { ProjectStatusTool } from "@/runtime/tools/builtins/project-status"
import { MemoryRecallTool } from "@/runtime/tools/builtins/memory-recall"
import { MemoryListTool } from "@/runtime/tools/builtins/memory-list"
import { ThreadReadTool, ThreadRecentTool, ThreadWaitTool } from "@/runtime/tools/builtins/thread-read"
import { ResourceTool } from "@/runtime/tools/builtins/resource"
import { VaultQueryTool } from "@/runtime/tools/builtins/vault-query"
import { VaultWriteTool } from "@/runtime/tools/builtins/vault-write"
import { Glob } from "@/shared/util/glob"
import { CreateGoalTool, GetGoalTool, SetGoalBudgetTool, UpdateGoalTool } from "@/runtime/tools/builtins/goal"
import { AgentSwarmTool } from "@/runtime/tools/builtins/agent-swarm"
import { AgentBatchTool } from "@/runtime/tools/builtins/agent-batch"
import { SleepTool } from "@/runtime/tools/builtins/sleep"
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
import { SuggestTaskTool } from "@/runtime/tools/builtins/suggest-task"
import { BotCreateTool, VendorConnectTool } from "@/runtime/tools/builtins/worker-bot-tools"
import { ProjectListTool, ProjectSendTool } from "@/runtime/tools/builtins/project-send"
import { GrantTaskPermissionTool } from "@/runtime/tools/builtins/grant-task-permission"
import { ThreadCloseTool, ThreadSearchTool, ThreadUpdateTool } from "@/runtime/tools/builtins/thread-tools"
import { MdxGraphTool } from "@/runtime/tools/builtins/mdx-graph"
import { subscriptionCapabilityTools } from "@/runtime/tools/builtins/subscription"

export namespace ToolRegistry {
  const log = Log.create({ service: "tool.registry" })

  /**
   * Thread tools that steer the calling chat's own thread: SessionPrompt.resolveTools
   * deletes them outside an app bot's chat. thread_search and the past-chat tools
   * (thread_read, thread_recent, thread_wait) are not here: every session gets them.
   */
  export const BOT_THREAD_TOOL_IDS = [ThreadUpdateTool.id, ThreadCloseTool.id] as const

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

  /** Per-turn additions to the tool list. */
  export interface TurnOptions {
    /** The turn carries `metadata.tools.computer: true` (pet offers): the
     * computer toolset is on for this turn even without the env flag. */
    computer?: boolean
  }

  async function all(turn?: TurnOptions): Promise<Tool.Info[]> {
    const custom = await state().then((x) => x.custom)
    const config = await Config.get()
    const question = ["app", "cli", "desktop"].includes(Flag.GIZZI_CLIENT) || Flag.GIZZI_ENABLE_QUESTION_TOOL
    // An app client has a view to capture and a pane to open results in; a
    // terminal (cli) has neither.
    const appClient = ["app", "desktop"].includes(Flag.GIZZI_CLIENT)
    // Hidden browser tasks need a local executor with its browser driver up
    // (a remote company API is never one). Reads the last background check only,
    // so resolving tools never waits on the executor; unknown counts as off.
    const browserTasks = appClient && browserTasksAvailableNow()

    return [
      InvalidTool,
      ...(question ? [QuestionTool] : []),
      // Acts on the page shown in the app's browser pane; needs an app client.
      ...(question ? [PaneBrowserTool] : []),
      // Sees what the user sees in the app (asks once per session); app clients only.
      ...(appClient ? [CaptureScreenContextTool] : []),
      // Parallel hidden browser tasks on the user's own machine (each call asks).
      ...(browserTasks ? [BrowserTasksTool] : []),
      // Works on the document open in the app's artifact pane; needs an app client.
      ...(question ? [PaneArtifactTool] : []),
      // Images (native lane renders in the app); needs an app client.
      ...(question ? [MediaGenerateTool] : []),
      // Looks at an artifact at each width (the app renders it); needs an app client.
      ...(question && !Flag.GIZZI_DISABLE_ARTIFACT_TOOLS ? [ArtifactCheckTool] : []),
      // Artifacts v2 (artifact_create/update/read): every client. They save to
      // the cloud store when this process has a credential; otherwise the
      // result carries the full payload and the app persists it.
      ...(Flag.GIZZI_DISABLE_ARTIFACT_TOOLS ? [] : ARTIFACT_TOOLS),
      AgentCommunicateTool,
      VerifyTool,
      BashTool,
      // Python on attached files, OS-sandboxed with network off (Chat analysis).
      RunCodeTool,
      ReadTool,
      GlobTool,
      GrepTool,
      ListTool,
      EditTool,
      MultiEditTool,
      SymbolEditTool,
      WriteTool,
      TaskTool,
      AgentSwarmTool,
      // One worker per CSV row with an output schema (sample first, resumable):
      // every client, not bot-gated. Asks with the row count, concurrency and
      // model; its workers get no task/agent_swarm/agent_batch (WORKER_DENIED_TOOLS).
      AgentBatchTool,
      BackgroundTaskListTool,
      BackgroundTaskOutputTool,
      BackgroundTaskStopTool,
      // Primitives Wave B: every client, not bot-gated. wait_agents and
      // agent_control act on the session's own descendants only.
      WaitAgentsTool,
      AgentControlTool,
      // Loads deferred tool families (primitives/tool-families.ts); replaces select_tools.
      ToolSearchTool,
      GetContextRemainingTool,
      HistoryRecallTool,
      ...(Flag.GIZZI_DISABLE_SCRATCHPAD
        ? []
        : [ScratchpadListTool, ScratchpadReadTool, ScratchpadWriteTool, ScratchpadRemoveTool]),
      WebFetchTool,
      TodoWriteTool,
      GlanceTool,
      // The time where the user is: every client (not bot-gated).
      CurrentTimeTool,
      // One fact as text plus a card (weather, time, convert, link, places,
      // images): every client. Time and unit conversion are local; network
      // kinds ask under websearch/webfetch.
      LookupTool,
      // A message (files, optional push) to the session's own user: every client, not bot-gated.
      SendUserMessageTool,
      // The user's scheduled tasks: every client, not bot-gated (list allowed, writes ask).
      AutomationTool,
      // A question that doesn't stop the turn; the answer starts a new one.
      // Every client, not bot-gated (routines, workers and subagents ask too).
      AskUserLaterTool,
      // A short in-turn wait that wakes on the user's next message: every client, not bot-gated.
      SleepTool,
      // Folders outside the project for this session: every client, not
      // bot-gated. Every call asks the person (an always-ask class).
      FolderAccessTool,
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
      ProjectStatusTool,
      MemoryRecallTool,
      // Wave D recall: memory topics/entries with versions, windowed reads of
      // past chats, and id-based views of large output. Every client, not bot-gated.
      MemoryListTool,
      ThreadReadTool,
      ThreadRecentTool,
      ThreadWaitTool,
      ResourceTool,
      VaultQueryTool,
      VaultWriteTool,
      SendAgentEmailTool,
      GetAgentEmailStatusTool,
      // Bot Mode (B4/D5): listed unconditionally — the registry has no session
      // context. SessionPrompt.resolveTools deletes it from the per-session
      // tool record unless the session is a canonical bot chat.
      MessageAgentTool,
      SuggestTaskTool,
      GrantTaskPermissionTool,
      // Thread tools (W3): thread_update / thread_close are gated like
      // suggest_task (app bot chats only). thread_search reaches every session
      // (Wave D decision 8, with the past-chat tools above).
      ThreadUpdateTool,
      ThreadCloseTool,
      ThreadSearchTool,
      // Worker bots + vendor connect: gated like suggest_task (app bot chats only).
      BotCreateTool,
      VendorConnectTool,
      // Send work to another project: gated to a bot's main chat; every send asks the person.
      ProjectListTool,
      ProjectSendTool,
      ...(Flag.GIZZI_ENABLE_LSP_TOOL ? [LspTool] : []),
      ...(config.experimental?.batch_tool === true ? [BatchTool] : []),
      ...(Flag.GIZZI_CLIENT === "cli" ? [PlanExitTool, PlanEnterTool] : []),
      // The computer toolset (allternit.browser.v1 / allternit.computer.v2):
      // one executor in allternit-api for every model. `browser` replaces the
      // old ACU-gateway browser tool under the same flag; `computer` drives
      // GIZZI_COMPUTER_ID (default this-device) and is opt-in; `computer_v2`
      // is the six structured driver-backed members (read_ui, act,
      // run_batch, verify, request_human, use_credential) every family gets
      // as a function tool.
      ...(Flag.GIZZI_ENABLE_BROWSER_TOOL ? [BrowserToolsetTool] : []),
      // `computers` (lifecycle + files via allternit-api computer_* tools)
      // rides the same opt-in as `computer`. A turn can also opt in on its own
      // (`metadata.tools.computer`, contracts/pet-offers): this turn only.
      ...(Flag.GIZZI_ENABLE_COMPUTER_TOOL || turn?.computer === true ? [ComputerToolsetTool, ComputerV2ToolsetTool, ComputersTool] : []),
      // Subscription capabilities (presentations, documents, deep research)
      // the user's connected subscriptions can run now; every call is
      // confirmed by the user (D16). Empty when the fabric is not set up.
      ...(await subscriptionCapabilityTools()),
      ...custom,
    ]
  }

  export async function ids(turn?: TurnOptions) {
    return all(turn).then((x) => x.map((t) => t.id))
  }

  export async function tools(
    model: {
      providerID: string
      modelID: string
      npm?: string
    },
    agent?: Agent.Info,
    sessionID?: string,
    turn?: TurnOptions,
  ) {
    const tools = await all(turn)
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
          const tool = await t.init({ agent, model, sessionID })
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
