import {
  RequestError,
  type Agent as ACPAgent,
  type AgentSideConnection,
  type AuthenticateRequest,
  type AuthMethod,
  type CancelNotification,
  type ForkSessionRequest,
  type ForkSessionResponse,
  type InitializeRequest,
  type InitializeResponse,
  type ListSessionsRequest,
  type ListSessionsResponse,
  type LoadSessionRequest,
  type NewSessionRequest,
  type PermissionOption,
  type PlanEntry,
  type PromptRequest,
  type ResumeSessionRequest,
  type ResumeSessionResponse,
  type Role,
  type SessionInfo,
  type SetSessionModelRequest,
  type SetSessionModeRequest,
  type SetSessionModeResponse,
  type ToolCallContent,
  type ToolKind,
  type Usage,
} from "@agentclientprotocol/sdk"

import { Log } from "@/shared/util/log"
import { pathToFileURL } from "bun"
import { Filesystem } from "@/shared/util/filesystem"
import { ACPSessionManager } from "@/runtime/integrations/acp/session"
import type { ACPConfig } from "@/runtime/integrations/acp/types"
import { Provider } from "@/runtime/providers/provider"
import { currentDefaultBrain, isPinnedModel } from "@/runtime/providers/default-brain"
import { retiredModelId } from "@/runtime/providers/discovery/subprocess"
import { Agent as AgentModule } from "@/runtime/loop/agent"
import { Installation } from "@/shared/installation"
import { MessageV2 } from "@/runtime/session/message-v2"
import { Config } from "@/runtime/context/config/config"
import { Todo } from "@/runtime/session/todo"
import { z } from "zod/v4"
import { LoadAPIKeyError } from "ai"
import type { AllternitClientLike } from "@/runtime/integrations/acp/types"
import { Session } from "@/runtime/session"
import { applyPatch } from "diff"
import { RuntimeTelemetry } from "@/runtime/telemetry"
import { Flag } from "@/runtime/context/flag/flag"

type ModeOption = { id: string; name: string; description?: string }
type ModelOption = { modelId: string; name: string }
type SessionConfigSelectOption = { value: string; name: string; description?: string }
type SessionConfigOption = {
  type: "select"
  id: "model" | "thinking" | "mode"
  name: string
  category: "model" | "thought_level" | "mode"
  currentValue: string
  options: SessionConfigSelectOption[]
}

// Define types for session message handling
interface SessionMessageResponse {
  info: {
    id: string
    sessionID: string
    role: "user" | "assistant" | string
    tokens: {
      input: number
      output: number
      reasoning: number
      cache?: {
        read?: number
        write?: number
      }
    }
    cost: number
    model?: {
      providerID: string
      modelID: string
    }
  }
  parts: Array<{
    id: string
    type: string
    callID?: string
    tool?: string
    state?: {
      status: "pending" | "running" | "completed" | "error"
      input?: Record<string, any>
      output?: string
      error?: string
      title?: string
      metadata?: Record<string, any>
    }
    text?: string
    url?: string
    filename?: string
    mime?: string
    synthetic?: boolean
    ignored?: boolean
  }>
}

interface AssistantMessage {
  tokens: {
    input: number
    output: number
    reasoning: number
    cache?: {
      read?: number
      write?: number
    }
  }
}

// Define Event types for handleEvent
interface PermissionAskedEvent {
  type: "permission.asked"
  properties: {
    id: string
    sessionID: string
    permission: string
    tool?: {
      callID?: string
    }
    metadata?: Record<string, any>
  }
}

interface MessagePartUpdatedEvent {
  type: "message.part.updated"
  properties: {
    part: {
      sessionID: string
      messageID: string
      callID: string
      tool: string
      type: string
      state: {
        status: "pending" | "running" | "completed" | "error"
        input?: Record<string, any>
        output?: string
        error?: string
        title?: string
        metadata?: Record<string, any>
      }
    }
  }
}

interface MessagePartDeltaEvent {
  type: "message.part.delta"
  properties: {
    sessionID: string
    messageID: string
    partID: string
    field: string
    delta: string
  }
}

type Event = PermissionAskedEvent | MessagePartUpdatedEvent | MessagePartDeltaEvent

const DEFAULT_VARIANT_VALUE = "default"

export namespace ACP {
  const log = Log.create({ service: "acp-agent" })

  async function getContextLimit(
    providerID: string,
    modelID: string,
  ): Promise<number | null> {
    const providers = await Provider.list()
    const provider = providers[providerID]
    const model = provider?.models?.[modelID] as any
    return model?.limit?.context ?? model?.context ?? null
  }

  async function sendUsageUpdate(
    connection: AgentSideConnection,
    sessionID: string,
  ): Promise<void> {
    const messages = await Session.messages({ sessionID }).catch((error: unknown) => {
      log.error("failed to fetch messages for usage update", { error })
      return undefined
    })

    if (!messages) return

    const assistantMessages = messages.filter(
      (m: any) => m.info?.role === "assistant",
    )

    const lastAssistant = assistantMessages[assistantMessages.length - 1]
    if (!lastAssistant) return

    const msg = lastAssistant.info
    const size = await getContextLimit((msg as any).providerID, (msg as any).modelID)

    if (!size) {
      // Cannot calculate usage without known context size
      return
    }

    const used = (msg as any).tokens?.input + ((msg as any).tokens?.cache?.read ?? 0)
    const totalCost = assistantMessages.reduce((sum, m) => sum + (m.info as any).cost, 0)

    await connection
      .sessionUpdate({
        sessionId: sessionID,
        update: {
          sessionUpdate: "usage_update",
          used,
          size,
          cost: { amount: totalCost, currency: "USD" },
        },
      })
      .catch((error: unknown) => {
        log.error("failed to send usage update", { error })
      })
  }

  export async function init({ sdk: _sdk }: { sdk: AllternitClientLike }) {
    return {
      create: (connection: AgentSideConnection, fullConfig: ACPConfig) => {
        return new Agent(connection, fullConfig)
      },
    }
  }

  export class Agent implements ACPAgent {
    private connection: AgentSideConnection
    private config: ACPConfig
    private sdk: AllternitClientLike
    private sessionManager: ACPSessionManager
    private eventAbort = new AbortController()
    private eventStarted = false
    private permissionQueues = new Map<string, Promise<void>>()
    private permissionOptions: PermissionOption[] = [
      { optionId: "once", kind: "allow_once", name: "Allow once" },
      { optionId: "always", kind: "allow_always", name: "Always allow" },
      { optionId: "reject", kind: "reject_once", name: "Reject" },
    ]

    constructor(connection: AgentSideConnection, config: ACPConfig) {
      this.connection = connection
      this.config = config
      this.sdk = config.sdk
      this.sessionManager = new ACPSessionManager(this.sdk as any)
      this.startEventSubscription()
    }

    private startEventSubscription() {
      if (this.eventStarted) return
      this.eventStarted = true
      this.runEventSubscription().catch((error: unknown) => {
        if (this.eventAbort.signal.aborted) return
        log.error("event subscription failed", { error })
      })
    }

    private async runEventSubscription() {
      while (true) {
        if (this.eventAbort.signal.aborted) return
        for await (const event of this.sdk.globalEvents({ signal: this.eventAbort.signal })) {
          if (this.eventAbort.signal.aborted) return
          await this.handleEvent(event as Event).catch((error: unknown) => {
            log.error("failed to handle event", { error, type: (event as any)?.type })
          })
        }
      }
    }

    private async handleEvent(event: Event) {
      switch (event.type) {
        case "permission.asked": {
          const permission = event.properties
          const session = this.sessionManager.tryGet(permission.sessionID)
          if (!session) return

          const prev = this.permissionQueues.get(permission.sessionID) ?? Promise.resolve()
          const next = prev
            .then(async () => {
              const directory = session.cwd

              const res = await this.connection
                .requestPermission({
                  sessionId: permission.sessionID,
                  toolCall: {
                    toolCallId: permission.tool?.callID ?? permission.id,
                    status: "pending",
                    title: permission.permission,
                    rawInput: permission.metadata,
                    kind: toToolKind(permission.permission),
                    locations: toLocations(permission.permission, permission.metadata),
                  },
                  options: this.permissionOptions,
                })
                .catch(async (error: unknown) => {
                  log.error("failed to request permission from ACP", {
                    error,
                    permissionID: permission.id,
                    sessionID: permission.sessionID,
                  })
                  await (this.sdk as any).permission.reply({
                    requestID: permission.id,
                    reply: "reject",
                    directory,
                  })
                  return undefined
                })

              if (!res) return
              if (res.outcome.outcome !== "selected") {
                await (this.sdk as any).permission.reply({
                  requestID: permission.id,
                  reply: "reject",
                  directory,
                })
                return
              }

              if (res.outcome.optionId !== "reject" && permission.permission == "edit") {
                const metadata = permission.metadata || {}
                const filepath = typeof metadata["filepath"] === "string" ? metadata["filepath"] : ""
                const diff = typeof metadata["diff"] === "string" ? metadata["diff"] : ""
                const content = (await Filesystem.exists(filepath)) ? await Filesystem.readText(filepath) : ""
                const newContent = getNewContent(content, diff)

                if (newContent) {
                  this.connection.writeTextFile({
                    sessionId: session.id,
                    path: filepath,
                    content: newContent,
                  })
                }
              }

              await (this.sdk as any).permission.reply({
                requestID: permission.id,
                reply: res.outcome.optionId as "once" | "always" | "reject",
                directory,
              })
            })
            .catch((error: unknown) => {
              log.error("failed to handle permission", { error, permissionID: permission.id })
            })
            .finally(() => {
              if (this.permissionQueues.get(permission.sessionID) === next) {
                this.permissionQueues.delete(permission.sessionID)
              }
            })
          this.permissionQueues.set(permission.sessionID, next)
          return
        }

        case "message.part.updated": {
          log.info("message part updated", { event: event.properties })
          const props = event.properties
          const part = props.part
          const session = this.sessionManager.tryGet(part.sessionID)
          if (!session) return
          const sessionId = session.id
          const directory = session.cwd

          const message = await this.sdk.session
            .messages({
              path: { sessionID: part.sessionID },
              query: { directory },
              throwOnError: true,
            })
            .then((x: any) => x.data?.[0])
            .catch((error: unknown) => {
              log.error("unexpected error when fetching message", { error })
              return undefined
            })

          if (!message || message.info.role !== "assistant") return

          if (part.type === "tool") {
            switch (part.state.status) {
              case "pending":
                await this.connection
                  .sessionUpdate({
                    sessionId,
                    update: {
                      sessionUpdate: "tool_call",
                      toolCallId: part.callID,
                      status: "pending",
                      title: part.tool,
                      kind: toToolKind(part.tool),
                      locations: [],
                      rawInput: {},
                    },
                  })
                  .catch((error: unknown) => {
                    log.error("failed to send tool pending to ACP", { error })
                  })
                return

              case "running":
                await this.connection
                  .sessionUpdate({
                    sessionId,
                    update: {
                      sessionUpdate: "tool_call_update",
                      toolCallId: part.callID,
                      status: "in_progress",
                      kind: toToolKind(part.tool),
                      title: part.tool,
                      locations: toLocations(part.tool, part.state.input),
                      rawInput: part.state.input,
                    },
                  })
                  .catch((error: unknown) => {
                    log.error("failed to send tool in_progress to ACP", { error })
                  })
                return

              case "completed": {
                const kind = toToolKind(part.tool)
                const content: ToolCallContent[] = [
                  {
                    type: "content",
                    content: {
                      type: "text",
                      text: part.state.output ?? "",
                    },
                  },
                ]

                if (kind === "edit") {
                  const input = part.state.input
                  const filePath = typeof input?.["filePath"] === "string" ? input["filePath"] : ""
                  const oldText = typeof input?.["oldString"] === "string" ? input["oldString"] : ""
                  const newText =
                    typeof input?.["newString"] === "string"
                      ? input["newString"]
                      : typeof input?.["content"] === "string"
                        ? input["content"]
                        : ""
                  content.push({
                    type: "diff",
                    path: filePath,
                    oldText,
                    newText,
                  })
                }

                if (part.tool === "todowrite") {
                  const parsedTodos = z.array(Todo.Info).safeParse(JSON.parse(part.state.output!))
                  if (parsedTodos.success) {
                    await this.connection
                      .sessionUpdate({
                        sessionId,
                        update: {
                          sessionUpdate: "plan",
                          entries: parsedTodos.data.map((todo) => {
                            const status: PlanEntry["status"] =
                              todo.status === "cancelled" ? "completed" : (todo.status as PlanEntry["status"])
                            return {
                              priority: "medium",
                              status,
                              content: todo.content,
                            }
                          }),
                        },
                      })
                      .catch((error: unknown) => {
                        log.error("failed to send session update for todo", { error })
                      })
                  } else {
                    log.error("failed to parse todo output", { error: parsedTodos.error })
                  }
                }

                await this.connection
                  .sessionUpdate({
                    sessionId,
                    update: {
                      sessionUpdate: "tool_call_update",
                      toolCallId: part.callID,
                      status: "completed",
                      kind,
                      content,
                      title: part.state.title,
                      rawInput: part.state.input,
                      rawOutput: {
                        output: part.state.output,
                        metadata: part.state.metadata,
                      },
                    },
                  })
                  .catch((error: unknown) => {
                    log.error("failed to send tool completed to ACP", { error })
                  })
                return
              }
              case "error":
                await this.connection
                  .sessionUpdate({
                    sessionId,
                    update: {
                      sessionUpdate: "tool_call_update",
                      toolCallId: part.callID,
                      status: "failed",
                      kind: toToolKind(part.tool),
                      title: part.tool,
                      rawInput: part.state.input,
                      content: [
                        {
                          type: "content",
                          content: {
                            type: "text",
                            text: part.state.error ?? "",
                          },
                        },
                      ],
                      rawOutput: {
                        error: part.state.error,
                      },
                    },
                  })
                  .catch((error: unknown) => {
                    log.error("failed to send tool error to ACP", { error })
                  })
                return
            }
          }
          return
        }

        case "message.part.delta": {
          const props = event.properties
          const session = this.sessionManager.tryGet(props.sessionID)
          if (!session) return
          const sessionId = session.id

          const message = await this.sdk.session
            .messages({
              path: { sessionID: props.sessionID },
              query: { directory: session.cwd },
              throwOnError: true,
            })
            .then((x: any) => x.data?.[0])
            .catch((error: unknown) => {
              log.error("unexpected error when fetching message", { error })
              return undefined
            })

          if (!message || message.info.role !== "assistant") return

          const part = message.parts.find((p: any) => p.id === props.partID)
          if (!part) return

          if (part.type === "text" && props.field === "text" && part.ignored !== true) {
            await this.connection
              .sessionUpdate({
                sessionId,
                update: {
                  sessionUpdate: "agent_message_chunk",
                  content: {
                    type: "text",
                    text: props.delta,
                  },
                },
              })
              .catch((error: unknown) => {
                log.error("failed to send text delta to ACP", { error })
              })
            return
          }

          if (part.type === "reasoning" && props.field === "text") {
            await this.connection
              .sessionUpdate({
                sessionId,
                update: {
                  sessionUpdate: "agent_thought_chunk",
                  content: {
                    type: "text",
                    text: props.delta,
                  },
                },
              })
              .catch((error: unknown) => {
                log.error("failed to send reasoning delta to ACP", { error })
              })
          }
          return
        }
      }
    }

    async initialize(params: InitializeRequest): Promise<InitializeResponse> {
      log.info("initialize", { protocolVersion: params.protocolVersion })

      const authMethod: AuthMethod = {
        description: "Run `gizzi auth login` in the terminal",
        name: "Login with gizzi",
        id: "gizzi-login",
      }

      // If client supports terminal-auth capability, use that instead.
      if (params.clientCapabilities?._meta?.["terminal-auth"] === true) {
        authMethod._meta = {
          "terminal-auth": {
            command: "gizzi",
            args: ["auth", "login"],
            label: "GIZZI Login",
          },
        }
      }

      return {
        protocolVersion: 1,
        agentCapabilities: {
          loadSession: true,
          mcpCapabilities: {
            http: true,
            sse: true,
          },
          promptCapabilities: {
            embeddedContext: true,
            image: true,
          },
          sessionCapabilities: {
            fork: {},
            list: {},
            resume: {},
          },
        },
        authMethods: [authMethod],
        agentInfo: {
          name: "GIZZI",
          version: Installation.VERSION,
        },
      }
    }

    async authenticate(_params: AuthenticateRequest) {
      // Verify the client has valid auth by checking configured providers
      const providers = await Provider.list()
      const hasAuth = Object.values(providers).some((p: any) => p.models && Object.keys(p.models).length > 0)
      if (!hasAuth) {
        throw new Error(
          "No authenticated providers found. Run `gizzi connect login` to set up authentication.",
        )
      }
      // ACP auth is validated — the client can proceed to create sessions.
      // Provider-level auth (API keys) is checked at model invocation time.
    }

    async newSession(params: NewSessionRequest) {
      const directory = params.cwd
      try {
        const model = await defaultModel(this.config, directory)

        // Store ACP session state
        const state = await this.sessionManager.create(params.cwd, params.mcpServers, model)
        const sessionId = state.id

        log.info("creating_session", { sessionId, mcpServers: params.mcpServers.length })

        const load = await this.loadSessionMode({
          cwd: directory,
          mcpServers: params.mcpServers,
          sessionId,
        })

        return {
          sessionId,
          models: load.models,
          modes: load.modes,
          configOptions: load.configOptions,
          _meta: load._meta,
        }
      } catch (e) {
        const error = MessageV2.fromError(e, {
          providerID: this.config.defaultModel?.providerID ?? "unknown",
        })
        if (LoadAPIKeyError.isInstance(error)) {
          throw RequestError.authRequired()
        }
        throw e
      }
    }

    async loadSession(params: LoadSessionRequest) {
      const directory = params.cwd
      const sessionId = params.sessionId

      try {
        const model = await defaultModel(this.config, directory)

        // Store ACP session state
        await this.sessionManager.load(sessionId, params.cwd, params.mcpServers, model)

        log.info("load_session", { sessionId, mcpServers: params.mcpServers.length })

        const result = await this.loadSessionMode({
          cwd: directory,
          mcpServers: params.mcpServers,
          sessionId,
        })

        // Replay session history
        const messages = await this.sdk.session
          .messages({
            path: { sessionID: sessionId },
            query: { directory },
            throwOnError: true,
          })
          .then((x: any) => x.data)
          .catch((err: unknown) => {
            log.error("unexpected error when fetching message", { error: err })
            return undefined
          })

        const lastUser = messages?.findLast((m: any) => m.info.role === "user")?.info
        if (lastUser?.role === "user") {
          result.models.currentModelId = `${lastUser.model.providerID}/${lastUser.model.modelID}`
          this.sessionManager.setModel(sessionId, {
            providerID: lastUser.model.providerID,
            modelID: lastUser.model.modelID,
          })
          if (result.modes?.availableModes.some((m: any) => m.id === lastUser.agent)) {
            result.modes.currentModeId = lastUser.agent
            this.sessionManager.setMode(sessionId, lastUser.agent)
          }
          result.configOptions = await this.refreshSessionConfigOptions(sessionId)
        }

        for (const msg of messages ?? []) {
          log.debug("replay message", msg)
          await this.processMessage(msg)
        }

        await sendUsageUpdate(this.connection, sessionId)

        return result
      } catch (e) {
        const error = MessageV2.fromError(e, {
          providerID: this.config.defaultModel?.providerID ?? "unknown",
        })
        if (LoadAPIKeyError.isInstance(error)) {
          throw RequestError.authRequired()
        }
        throw e
      }
    }

    async unstable_listSessions(params: ListSessionsRequest): Promise<ListSessionsResponse> {
      try {
        const cursor = params.cursor ? Number(params.cursor) : undefined
        const limit = 100

        const sessions = await this.sdk.session
          .list({
            query: { directory: params.cwd ?? undefined },
            throwOnError: true,
          })
          .then((x: any) => x.data ?? [])

        const sorted = sessions.slice().sort((a: any, b: any) => b.time.updated - a.time.updated)
        const filtered = cursor ? sorted.filter((s: any) => s.time.updated < cursor) : sorted
        const page = filtered.slice(0, limit)

        const entries: SessionInfo[] = page.map((session: any) => ({
          sessionId: session.id,
          cwd: session.directory,
          title: session.title,
          updatedAt: new Date(session.time.updated).toISOString(),
        }))

        const last = page[page.length - 1]
        const next = filtered.length > limit && last ? String(last.time.updated) : undefined

        const response: ListSessionsResponse = {
          sessions: entries,
        }
        if (next) response.nextCursor = next
        return response
      } catch (e) {
        const error = MessageV2.fromError(e, {
          providerID: this.config.defaultModel?.providerID ?? "unknown",
        })
        if (LoadAPIKeyError.isInstance(error)) {
          throw RequestError.authRequired()
        }
        throw e
      }
    }

    async unstable_forkSession(params: ForkSessionRequest): Promise<ForkSessionResponse> {
      const directory = params.cwd
      const mcpServers = params.mcpServers ?? []

      try {
        const model = await defaultModel(this.config, directory)

        const forked = await this.sdk.session
          .fork({
            path: { sessionID: params.sessionId },
            query: { directory },
            throwOnError: true,
          })
          .then((x: any) => x.data)

        if (!forked) {
          throw new Error("Fork session returned no data")
        }

        const sessionId = forked.id
        await this.sessionManager.load(sessionId, directory, mcpServers, model)

        log.info("fork_session", { sessionId, mcpServers: mcpServers.length })

        const mode = await this.loadSessionMode({
          cwd: directory,
          mcpServers,
          sessionId,
        })

        const messages = await this.sdk.session
          .messages({
            path: { sessionID: sessionId },
            query: { directory },
            throwOnError: true,
          })
          .then((x: any) => x.data)
          .catch((err: unknown) => {
            log.error("unexpected error when fetching message", { error: err })
            return undefined
          })

        for (const msg of messages ?? []) {
          log.debug("replay message", msg)
          await this.processMessage(msg)
        }

        await sendUsageUpdate(this.connection, sessionId)

        return mode
      } catch (e) {
        const error = MessageV2.fromError(e, {
          providerID: this.config.defaultModel?.providerID ?? "unknown",
        })
        if (LoadAPIKeyError.isInstance(error)) {
          throw RequestError.authRequired()
        }
        throw e
      }
    }

    async unstable_resumeSession(params: ResumeSessionRequest): Promise<ResumeSessionResponse> {
      const directory = params.cwd
      const sessionId = params.sessionId
      const mcpServers = params.mcpServers ?? []

      try {
        const model = await defaultModel(this.config, directory)
        await this.sessionManager.load(sessionId, directory, mcpServers, model)

        log.info("resume_session", { sessionId, mcpServers: mcpServers.length })

        const result = await this.loadSessionMode({
          cwd: directory,
          mcpServers,
          sessionId,
        })

        await sendUsageUpdate(this.connection, sessionId)

        return result
      } catch (e) {
        const error = MessageV2.fromError(e, {
          providerID: this.config.defaultModel?.providerID ?? "unknown",
        })
        if (LoadAPIKeyError.isInstance(error)) {
          throw RequestError.authRequired()
        }
        throw e
      }
    }

    private async processMessage(message: SessionMessageResponse) {
      log.debug("process message", message)
      if (message.info.role !== "assistant" && message.info.role !== "user") return
      const sessionId = message.info.sessionID

      for (const part of message.parts) {
        if (part.type === "tool") {
          switch (part.state?.status) {
            case "pending":
              await this.connection
                .sessionUpdate({
                  sessionId,
                  update: {
                    sessionUpdate: "tool_call",
                    toolCallId: part.callID!,
                    title: part.tool!,
                    kind: toToolKind(part.tool!),
                    status: "pending",
                    locations: [],
                    rawInput: {},
                  },
                })
                .catch((err: unknown) => {
                  log.error("failed to send tool pending to ACP", { error: err })
                })
              break
            case "running":
              await this.connection
                .sessionUpdate({
                  sessionId,
                  update: {
                    sessionUpdate: "tool_call_update",
                    toolCallId: part.callID!,
                    status: "in_progress",
                    kind: toToolKind(part.tool!),
                    title: part.tool!,
                    locations: toLocations(part.tool!, part.state?.input),
                    rawInput: part.state?.input,
                  },
                })
                .catch((err: unknown) => {
                  log.error("failed to send tool in_progress to ACP", { error: err })
                })
              break
            case "completed": {
              const kind = toToolKind(part.tool!)
              const content: ToolCallContent[] = [
                {
                  type: "content",
                  content: {
                    type: "text",
                    text: part.state?.output ?? "",
                  },
                },
              ]

              if (kind === "edit") {
                const input = part.state?.input
                const filePath = typeof input?.["filePath"] === "string" ? input["filePath"] : ""
                const oldText = typeof input?.["oldString"] === "string" ? input["oldString"] : ""
                const newText =
                  typeof input?.["newString"] === "string"
                    ? input["newString"]
                    : typeof input?.["content"] === "string"
                      ? input["content"]
                      : ""
                content.push({
                  type: "diff",
                  path: filePath,
                  oldText,
                  newText,
                })
              }

              if (part.tool === "todowrite" && part.state?.output) {
                const parsedTodos = z.array(Todo.Info).safeParse(JSON.parse(part.state.output))
                if (parsedTodos.success) {
                  await this.connection
                    .sessionUpdate({
                      sessionId,
                      update: {
                        sessionUpdate: "plan",
                        entries: parsedTodos.data.map((todo) => {
                          const status: PlanEntry["status"] =
                            todo.status === "cancelled" ? "completed" : (todo.status as PlanEntry["status"])
                          return {
                            priority: "medium",
                            status,
                            content: todo.content,
                          }
                        }),
                      },
                    })
                    .catch((err: unknown) => {
                      log.error("failed to send session update for todo", { error: err })
                    })
                } else {
                  log.error("failed to parse todo output", { error: parsedTodos.error })
                }
              }

              await this.connection
                .sessionUpdate({
                  sessionId,
                  update: {
                    sessionUpdate: "tool_call_update",
                    toolCallId: part.callID!,
                    status: "completed",
                    kind,
                    content,
                    title: part.state?.title,
                    rawInput: part.state?.input,
                    rawOutput: {
                      output: part.state?.output,
                      metadata: part.state?.metadata,
                    },
                  },
                })
                .catch((err: unknown) => {
                  log.error("failed to send tool completed to ACP", { error: err })
                })
              break
            }
            case "error":
              await this.connection
                .sessionUpdate({
                  sessionId,
                  update: {
                    sessionUpdate: "tool_call_update",
                    toolCallId: part.callID!,
                    status: "failed",
                    kind: toToolKind(part.tool!),
                    title: part.tool!,
                    rawInput: part.state?.input,
                    content: [
                      {
                        type: "content",
                        content: {
                          type: "text",
                          text: part.state?.error ?? "",
                        },
                      },
                    ],
                    rawOutput: {
                      error: part.state?.error ?? undefined,
                    },
                  },
                })
                .catch((err: unknown) => {
                  log.error("failed to send tool error to ACP", { error: err })
                })
              break
          }
        } else if (part.type === "text") {
          if (part.text) {
            const audience: Role[] | undefined = part.synthetic ? ["assistant"] : part.ignored ? ["user"] : undefined
            await this.connection
              .sessionUpdate({
                sessionId,
                update: {
                  sessionUpdate: message.info.role === "user" ? "user_message_chunk" : "agent_message_chunk",
                  content: {
                    type: "text",
                    text: part.text,
                    ...(audience && { annotations: { audience } }),
                  },
                },
              })
              .catch((err: unknown) => {
                log.error("failed to send text to ACP", { error: err })
              })
          }
        } else if (part.type === "file") {
          // Replay file attachments as appropriate ACP content blocks.
          // GIZZI stores files internally as { type: "file", url, filename, mime }.
          // We convert these back to ACP blocks based on the URL scheme and MIME type:
          // - file:// URLs → resource_link
          // - data: URLs with image/* → image block
          // - data: URLs with text/* or application/json → resource with text
          // - data: URLs with other types → resource with blob
          const url = part.url
          const filename = part.filename ?? "file"
          const mime = part.mime || "application/octet-stream"
          const messageChunk = message.info.role === "user" ? "user_message_chunk" : "agent_message_chunk"

          if (url?.startsWith("file://")) {
            // Local file reference - send as resource_link
            await this.connection
              .sessionUpdate({
                sessionId,
                update: {
                  sessionUpdate: messageChunk,
                  content: { type: "resource_link", uri: url, name: filename, mimeType: mime },
                },
              })
              .catch((err: unknown) => {
                log.error("failed to send resource_link to ACP", { error: err })
              })
          } else if (url?.startsWith("data:")) {
            // Embedded content - parse data URL and send as appropriate block type
            const base64Match = url.match(/^data:([^;]+);base64,(.*)$/)
            const dataMime = base64Match?.[1]
            const base64Data = base64Match?.[2] ?? ""

            const effectiveMime = dataMime || mime

            if (effectiveMime.startsWith("image/")) {
              // Image - send as image block
              await this.connection
                .sessionUpdate({
                  sessionId,
                  update: {
                    sessionUpdate: messageChunk,
                    content: {
                      type: "image",
                      mimeType: effectiveMime,
                      data: base64Data,
                      uri: pathToFileURL(filename).href,
                    },
                  },
                })
                .catch((err: unknown) => {
                  log.error("failed to send image to ACP", { error: err })
                })
            } else {
              // Non-image: text types get decoded, binary types stay as blob
              const isText = effectiveMime.startsWith("text/") || effectiveMime === "application/json"
              const fileUri = pathToFileURL(filename).href
              const resource = isText
                ? {
                    uri: fileUri,
                    mimeType: effectiveMime,
                    text: Buffer.from(base64Data, "base64").toString("utf-8"),
                  }
                : { uri: fileUri, mimeType: effectiveMime, blob: base64Data }

              await this.connection
                .sessionUpdate({
                  sessionId,
                  update: {
                    sessionUpdate: messageChunk,
                    content: { type: "resource", resource },
                  },
                })
                .catch((err: unknown) => {
                  log.error("failed to send resource to ACP", { error: err })
                })
            }
          }
          // URLs that don't match file:// or data: are skipped (unsupported)
        } else if (part.type === "reasoning") {
          if (part.text) {
            await this.connection
              .sessionUpdate({
                sessionId,
                update: {
                  sessionUpdate: "agent_thought_chunk",
                  content: {
                    type: "text",
                    text: part.text,
                  },
                },
              })
              .catch((err: unknown) => {
                log.error("failed to send reasoning to ACP", { error: err })
              })
          }
        }
      }
    }

    private async loadAvailableModes(directory: string): Promise<ModeOption[]> {
      const agents = await this.config.sdk.agent
        .list({
          query: { directory },
          throwOnError: true,
        })
        .then((resp: any) => resp.data!)

      return agents
        .filter((agent: any) => agent.mode !== "subagent" && !agent.hidden)
        .map((agent: any) => ({
          id: agent.name,
          name: agent.name,
          description: agent.description,
        }))
    }

    private async resolveModeState(
      directory: string,
      sessionId: string,
    ): Promise<{ availableModes: ModeOption[]; currentModeId?: string }> {
      const availableModes = await this.loadAvailableModes(directory)
      const currentModeId =
        this.sessionManager.get(sessionId).modeId ||
        (await (async () => {
          if (!availableModes.length) return undefined
          const defaultAgentName = await AgentModule.defaultAgent()
          const resolvedModeId =
            availableModes.find((mode) => mode.name === defaultAgentName)?.id ?? availableModes[0].id
          this.sessionManager.setMode(sessionId, resolvedModeId)
          return resolvedModeId
        })())

      return { availableModes, currentModeId }
    }

    private async loadSessionMode(params: LoadSessionRequest) {
      const directory = params.cwd
      const model = await defaultModel(this.config, directory)
      const sessionId = params.sessionId

      const providers = await this.sdk.config.providers({ query: { directory }, throwOnError: true }).then((x: any) => x.data!.providers)
      const entries = sortProvidersByName(providers as any[])
      const availableVariants = modelVariantsFromProviders(entries as any[], model)
      const currentVariant = this.sessionManager.getVariant(sessionId)
      if (currentVariant && !availableVariants.includes(currentVariant)) {
        this.sessionManager.setVariant(sessionId, undefined)
      }
      const availableModels = buildAvailableModels(entries as any[], { includeVariants: true })
      const modeState = await this.resolveModeState(directory, sessionId)
      const currentModeId = modeState.currentModeId
      const modes = currentModeId
        ? {
            availableModes: modeState.availableModes,
            currentModeId,
          }
        : undefined

      const commands = await (this.config.sdk as any).command
        ?.list({
          query: { directory },
          throwOnError: true,
        })
        .then((resp: any) => resp.data!) ?? []

      const availableCommands = commands.map((command: any) => ({
        name: command.name,
        description: command.description ?? "",
      }))
      const names = new Set(availableCommands.map((c: any) => c.name))
      if (!names.has("compact"))
        availableCommands.push({
          name: "compact",
          description: "compact the session",
        })

      const mcpServers: Record<string, Config.Mcp> = {}
      for (const server of params.mcpServers) {
        if ("type" in server) {
          mcpServers[server.name] = {
            url: server.url,
            headers: server.headers.reduce<Record<string, string>>((acc, { name, value }) => {
              acc[name] = value
              return acc
            }, {}),
            type: "remote",
          }
        } else {
          mcpServers[server.name] = {
            type: "local",
            command: [server.command, ...server.args],
            environment: server.env.reduce<Record<string, string>>((acc, { name, value }) => {
              acc[name] = value
              return acc
            }, {}),
          }
        }
      }

      await Promise.all(
        Object.entries(mcpServers).map(async ([key, mcp]) => {
          await this.sdk.mcp
            .add({
              query: { directory },
              body: { name: key, config: mcp },
              throwOnError: true,
            })
            .catch((error: unknown) => {
              log.error("failed to add mcp server", { name: key, error })
            })
        }),
      )

      setTimeout(() => {
        this.connection.sessionUpdate({
          sessionId,
          update: {
            sessionUpdate: "available_commands_update",
            availableCommands,
          },
        })
      }, 0)

      return {
        sessionId,
        models: {
          currentModelId: formatModelIdWithVariant(model, currentVariant, availableVariants, true),
          availableModels,
        },
        modes,
        configOptions: Flag.GIZZI_DISABLE_ACP_CONFIG_OPTIONS ? undefined : buildSessionConfigOptions({
          models: buildAvailableModels(entries as any[]),
          model,
          variants: availableVariants,
          variant: this.sessionManager.getVariant(sessionId),
          modes: modeState.availableModes,
          modeId: currentModeId,
        }),
        _meta: buildVariantMeta({
          model,
          variant: this.sessionManager.getVariant(sessionId),
          availableVariants,
        }),
      }
    }

    async unstable_setSessionModel(params: SetSessionModelRequest) {
      const session = this.sessionManager.get(params.sessionId)
      const providers = await this.sdk.config
        .providers({ query: { directory: session.cwd }, throwOnError: true })
        .then((x: any) => x.data!.providers)

      const selection = parseModelSelection(params.modelId, providers)
      this.sessionManager.setModel(session.id, selection.model)
      this.sessionManager.setVariant(session.id, selection.variant)

      const entries = sortProvidersByName(providers as any[])
      const availableVariants = modelVariantsFromProviders(entries as any[], selection.model)

      const configOptions = await this.refreshSessionConfigOptions(session.id)
      await this.emitConfigOptions(session.id, configOptions)
      RuntimeTelemetry.track("acp_config_changed", { config_id: "model" })
      return {
        configOptions,
        _meta: buildVariantMeta({
          model: selection.model,
          variant: selection.variant,
          availableVariants,
        }),
      }
    }

    async setSessionMode(params: SetSessionModeRequest): Promise<SetSessionModeResponse | void> {
      const session = this.sessionManager.get(params.sessionId)
      const availableModes = await this.loadAvailableModes(session.cwd)
      if (!availableModes.some((mode) => mode.id === params.modeId)) {
        throw new Error(`Agent not found: ${params.modeId}`)
      }
      this.sessionManager.setMode(params.sessionId, params.modeId)
      const configOptions = await this.refreshSessionConfigOptions(params.sessionId)
      await this.emitConfigOptions(params.sessionId, configOptions)
      RuntimeTelemetry.track("acp_config_changed", { config_id: "mode" })
      return { configOptions } as any
    }

    async setSessionConfigOption(params: {
      sessionId: string
      configId: string
      value: unknown
    }): Promise<{ configOptions: SessionConfigOption[] }> {
      if (Flag.GIZZI_DISABLE_ACP_CONFIG_OPTIONS) {
        throw RequestError.invalidParams(JSON.stringify({ error: "ACP config options are disabled by rollout policy" }))
      }
      const value = String(params.value)
      switch (params.configId) {
        case "model": {
          const result = await this.unstable_setSessionModel({ sessionId: params.sessionId, modelId: value })
          return { configOptions: (result as any).configOptions }
        }
        case "mode": {
          const result = await this.setSessionMode({ sessionId: params.sessionId, modeId: value })
          return { configOptions: (result as any).configOptions }
        }
        case "thinking": {
          const session = this.sessionManager.get(params.sessionId)
          const providers = await this.sdk.config
            .providers({ query: { directory: session.cwd }, throwOnError: true })
            .then((x: any) => x.data!.providers)
          const model = session.model ?? (await defaultModel(this.config, session.cwd))
          const variants = modelVariantsFromProviders(sortProvidersByName(providers as any[]), model)
          if (value !== "off" && !variants.includes(value)) {
            throw RequestError.invalidParams(JSON.stringify({ error: `Unknown thinking level: ${value}` }))
          }
          this.sessionManager.setVariant(params.sessionId, value === "off" ? undefined : value)
          RuntimeTelemetry.track("acp_config_changed", { config_id: "thinking" })
          break
        }
        default:
          throw RequestError.invalidParams(JSON.stringify({ error: `Unknown config option: ${params.configId}` }))
      }

      const configOptions = await this.refreshSessionConfigOptions(params.sessionId)
      await this.emitConfigOptions(params.sessionId, configOptions)
      return { configOptions }
    }

    private async refreshSessionConfigOptions(sessionId: string): Promise<SessionConfigOption[]> {
      if (Flag.GIZZI_DISABLE_ACP_CONFIG_OPTIONS) return []
      const session = this.sessionManager.get(sessionId)
      const model = session.model ?? (await defaultModel(this.config, session.cwd))
      const providers = await this.sdk.config
        .providers({ query: { directory: session.cwd }, throwOnError: true })
        .then((x: any) => x.data!.providers)
      const entries = sortProvidersByName(providers as any[])
      const modes = await this.resolveModeState(session.cwd, sessionId)
      return buildSessionConfigOptions({
        models: buildAvailableModels(entries),
        model,
        variants: modelVariantsFromProviders(entries as any[], model),
        variant: this.sessionManager.getVariant(sessionId),
        modes: modes.availableModes,
        modeId: modes.currentModeId,
      })
    }

    private async emitConfigOptions(sessionId: string, configOptions: SessionConfigOption[]) {
      if (Flag.GIZZI_DISABLE_ACP_CONFIG_OPTIONS) return
      await this.connection
        .sessionUpdate({
          sessionId,
          update: {
            sessionUpdate: "config_option_update",
            configOptions,
          } as any,
        })
        .catch((error: unknown) => log.error("failed to send ACP config options", { error, sessionId }))
    }

    async prompt(params: PromptRequest) {
      const sessionID = params.sessionId
      const session = this.sessionManager.get(sessionID)
      const directory = session.cwd

      const current = session.model
      const model = current ?? (await defaultModel(this.config, directory))
      if (!current) {
        this.sessionManager.setModel(session.id, model)
      }
      const agent = session.modeId ?? (await AgentModule.defaultAgent())

      const parts: Array<
        { type: "text"; text: string; synthetic?: boolean; ignored?: boolean } |
        { type: "file"; url: string; filename: string; mime: string }
      > = []
      for (const part of params.prompt) {
        switch (part.type) {
          case "text":
            const audience = part.annotations?.audience
            const forAssistant = audience?.length === 1 && audience[0] === "assistant"
            const forUser = audience?.length === 1 && audience[0] === "user"
            parts.push({
              type: "text" as const,
              text: part.text,
              ...(forAssistant && { synthetic: true }),
              ...(forUser && { ignored: true }),
            })
            break
          case "image": {
            const parsed = parseUri(part.uri ?? "")
            const filename = parsed.type === "file" ? parsed.filename : "image"
            if (part.data) {
              parts.push({
                type: "file",
                url: `data:${part.mimeType};base64,${part.data}`,
                filename,
                mime: part.mimeType,
              })
            } else if (part.uri && part.uri.startsWith("http:")) {
              parts.push({
                type: "file",
                url: part.uri,
                filename,
                mime: part.mimeType,
              })
            }
            break
          }

          case "resource_link":
            const parsed = parseUri(part.uri)
            // Use the name from resource_link if available
            if (part.name && parsed.type === "file") {
              parsed.filename = part.name
            }
            parts.push(parsed)

            break

          case "resource": {
            const resource = part.resource
            if ("text" in resource && resource.text) {
              parts.push({
                type: "text",
                text: resource.text,
              })
            } else if ("blob" in resource && resource.blob && resource.mimeType) {
              // Binary resource (PDFs, etc.): store as file part with data URL
              const parsed = parseUri(resource.uri ?? "")
              const filename = parsed.type === "file" ? parsed.filename : "file"
              parts.push({
                type: "file",
                url: `data:${resource.mimeType};base64,${resource.blob}`,
                filename,
                mime: resource.mimeType,
              })
            }
            break
          }

          default:
            break
        }
      }

      log.info("parts", { parts })

      const cmd = (() => {
        const text = parts
          .filter((p): p is { type: "text"; text: string } => p.type === "text")
          .map((p) => p.text)
          .join("")
          .trim()

        if (!text.startsWith("/")) return

        const [name, ...rest] = text.slice(1).split(/\s+/)
        return { name, args: rest.join(" ").trim() }
      })()

      const buildUsage = (msg: AssistantMessage): Usage => ({
        totalTokens:
          msg.tokens.input +
          msg.tokens.output +
          msg.tokens.reasoning +
          (msg.tokens.cache?.read ?? 0) +
          (msg.tokens.cache?.write ?? 0),
        inputTokens: msg.tokens.input,
        outputTokens: msg.tokens.output,
        thoughtTokens: msg.tokens.reasoning || undefined,
        cachedReadTokens: msg.tokens.cache?.read || undefined,
        cachedWriteTokens: msg.tokens.cache?.write || undefined,
      })

      if (!cmd) {
        const response = await this.sdk.session.prompt({
          path: { sessionID },
          body: {
            model: {
              providerID: model.providerID,
              modelID: model.modelID,
            },
            parts,
            agent,
          } as any,
        })
        const msg = response.data?.info

        await sendUsageUpdate(this.connection, sessionID)

        return {
          stopReason: "end_turn" as const,
          usage: msg ? buildUsage(msg) : undefined,
          _meta: {},
        }
      }

      const command = await (this.config.sdk as any).command
        ?.list({ query: { directory }, throwOnError: true })
        .then((x: any) => x.data?.find((c: any) => c.name === cmd.name))
      if (command) {
        const response = await this.sdk.session.command({
          path: { sessionID },
          body: {
            command: command.name,
            arguments: cmd.args,
            model: model.providerID + "/" + model.modelID,
            agent,
          } as any,
        })
        const msg = response.data?.info

        await sendUsageUpdate(this.connection, sessionID)

        return {
          stopReason: "end_turn" as const,
          usage: msg ? buildUsage(msg) : undefined,
          _meta: {},
        }
      }

      switch (cmd.name) {
        case "compact":
          await this.config.sdk.session.summarize({
            path: { sessionID },
            body: {
              providerID: model.providerID,
              modelID: model.modelID,
            },
            throwOnError: true,
          })
          break
      }

      await sendUsageUpdate(this.connection, sessionID)

      return {
        stopReason: "end_turn" as const,
        _meta: {},
      }
    }

    async cancel(params: CancelNotification) {
      const session = this.sessionManager.get(params.sessionId)
      await this.config.sdk.session.abort({
        path: { sessionID: params.sessionId },
        query: { directory: session.cwd },
        throwOnError: true,
      })
    }
  }

  function toToolKind(toolName: string): ToolKind {
    const tool = toolName.toLocaleLowerCase()
    switch (tool) {
      case "bash":
        return "execute"
      case "webfetch":
        return "fetch"

      case "edit":
      case "patch":
      case "write":
        return "edit"

      case "grep":
      case "glob":
      case "context7_resolve_library_id":
      case "context7_get_library_docs":
        return "search"

      case "list":
      case "read":
        return "read"

      default:
        return "other"
    }
  }

  function toLocations(toolName: string, input: Record<string, any> | undefined): { path: string }[] {
    const tool = toolName.toLocaleLowerCase()
    switch (tool) {
      case "read":
      case "edit":
      case "write":
        return input?.["filePath"] ? [{ path: input["filePath"] }] : []
      case "glob":
      case "grep":
        return input?.["path"] ? [{ path: input["path"] }] : []
      case "bash":
        return []
      case "list":
        return input?.["path"] ? [{ path: input["path"] }] : []
      default:
        return []
    }
  }

  /** A model ref with a retired model id mapped to its current one. */
  function currentRef(ref: { providerID: string; modelID: string }) {
    return { providerID: ref.providerID, modelID: retiredModelId(ref.providerID, ref.modelID) }
  }

  /**
   * The session's starting model. An explicit model wins: the ACP client's
   * `defaultModel`, then a `/model` pin in this directory's config. Otherwise
   * the one default-brain resolver decides (default-brain.ts); when its pick
   * is not available here, the best model this directory offers.
   */
  async function defaultModel(config: ACPConfig, cwd?: string) {
    const sdk = config.sdk
    if (config.defaultModel) return currentRef(config.defaultModel)

    const directory = cwd ?? process.cwd()

    const pinned = await sdk.config
      .get({ query: { directory }, throwOnError: true })
      .then((resp: any) => {
        const cfg = resp.data
        if (!cfg?.model || !isPinnedModel({ modelAuto: cfg.model_auto, currentModel: cfg.model })) return undefined
        return currentRef(Provider.parseModel(cfg.model))
      })
      .catch((error: unknown) => {
        log.error("failed to load user config for default model", { error })
        return undefined
      })

    const providers = await sdk.config
      .providers({ query: { directory }, throwOnError: true })
      .then((x: any) => x.data?.providers ?? [])
      .catch((error: unknown) => {
        log.error("failed to list providers for default model", { error })
        return []
      })

    const available = (ref: { providerID: string; modelID: string }) =>
      !providers.length || Boolean(providers.find((p: any) => p.id === ref.providerID)?.models[ref.modelID])

    if (pinned && available(pinned)) return pinned

    const brain = await currentDefaultBrain().catch((error: unknown) => {
      log.error("failed to resolve the default brain", { error })
      return undefined
    })
    if (brain?.model) {
      const resolved = currentRef(Provider.parseModel(brain.model))
      if (resolved.providerID !== "auto" && available(resolved)) return resolved
    }

    const models = providers.flatMap((p: any) => Object.values(p.models))
    const [best] = Provider.sort(models as any)
    if (best) {
      return {
        providerID: (best as any).providerID,
        modelID: (best as any).id,
      }
    }

    if (pinned) return pinned

    // Nothing in this directory: the runtime's own default (throws a clear
    // "connect a provider" error when nothing is signed in).
    return currentRef(await Provider.defaultModel())
  }

  function parseUri(
    uri: string,
  ): { type: "file"; url: string; filename: string; mime: string } | { type: "text"; text: string } {
    try {
      if (uri.startsWith("file://")) {
        const path = uri.slice(7)
        const name = path.split("/").pop() || path
        return {
          type: "file",
          url: uri,
          filename: name,
          mime: "text/plain",
        }
      }
      if (uri.startsWith("zed://")) {
        const url = new URL(uri)
        const path = url.searchParams.get("path")
        if (path) {
          const name = path.split("/").pop() || path
          return {
            type: "file",
            url: pathToFileURL(path).href,
            filename: name,
            mime: "text/plain",
          }
        }
      }
      return {
        type: "text",
        text: uri,
      }
    } catch {
      return {
        type: "text",
        text: uri,
      }
    }
  }

  function getNewContent(fileOriginal: string, unifiedDiff: string): string | undefined {
    const result = applyPatch(fileOriginal, unifiedDiff)
    if (result === false) {
      log.error("Failed to apply unified diff (context mismatch)")
      return undefined
    }
    return result
  }

  function sortProvidersByName<T extends { name: string }>(providers: T[]): T[] {
    return [...providers].sort((a, b) => {
      const nameA = a.name.toLowerCase()
      const nameB = b.name.toLowerCase()
      if (nameA < nameB) return -1
      if (nameA > nameB) return 1
      return 0
    })
  }

  function modelVariantsFromProviders(
    providers: Array<{ id: string; models: Record<string, { variants?: Record<string, any> }> }>,
    model: { providerID: string; modelID: string },
  ): string[] {
    const provider = providers.find((entry) => entry.id === model.providerID)
    if (!provider) return []
    const modelInfo = provider.models[model.modelID]
    if (!modelInfo?.variants) return []
    return Object.keys(modelInfo.variants)
  }

  function buildAvailableModels(
    providers: Array<{ id: string; name: string; models: Record<string, any> }>,
    options: { includeVariants?: boolean } = {},
  ): ModelOption[] {
    const includeVariants = options.includeVariants ?? false
    return providers.flatMap((provider) => {
      const models = Provider.sort(Object.values(provider.models) as any)
      return models.flatMap((model: any) => {
        const base: ModelOption = {
          modelId: `${provider.id}/${model.id}`,
          name: `${provider.name}/${model.name}`,
        }
        if (!includeVariants || !model.variants) return [base]
        const variants = Object.keys(model.variants).filter((variant) => variant !== DEFAULT_VARIANT_VALUE)
        const variantOptions = variants.map((variant) => ({
          modelId: `${provider.id}/${model.id}/${variant}`,
          name: `${provider.name}/${model.name} (${variant})`,
        }))
        return [base, ...variantOptions]
      })
    })
  }

  export function buildSessionConfigOptions(input: {
    models: ModelOption[]
    model: { providerID: string; modelID: string }
    variants: string[]
    variant?: string
    modes: ModeOption[]
    modeId?: string
  }): SessionConfigOption[] {
    const currentModelId = `${input.model.providerID}/${input.model.modelID}`
    const result: SessionConfigOption[] = [
      {
        type: "select",
        id: "model",
        name: "Model",
        category: "model",
        currentValue: currentModelId,
        options: input.models.map((model) => ({ value: model.modelId, name: model.name })),
      },
    ]

    const variants = input.variants.filter((variant) => variant !== DEFAULT_VARIANT_VALUE)
    if (variants.length) {
      result.push({
        type: "select",
        id: "thinking",
        name: "Thinking",
        category: "thought_level",
        currentValue: input.variant && variants.includes(input.variant) ? input.variant : "off",
        options: [
          { value: "off", name: "Thinking Off" },
          ...variants.map((variant) => ({
            value: variant,
            name: variant === "on" ? "Thinking On" : `Thinking ${variant}`,
          })),
        ],
      })
    }

    if (input.modeId) {
      result.push({
        type: "select",
        id: "mode",
        name: "Mode",
        category: "mode",
        currentValue: input.modeId,
        options: input.modes.map((mode) => ({
          value: mode.id,
          name: mode.name,
          ...(mode.description ? { description: mode.description } : {}),
        })),
      })
    }
    return result
  }

  function formatModelIdWithVariant(
    model: { providerID: string; modelID: string },
    variant: string | undefined,
    availableVariants: string[],
    includeVariant: boolean,
  ) {
    const base = `${model.providerID}/${model.modelID}`
    if (!includeVariant || !variant || !availableVariants.includes(variant)) return base
    return `${base}/${variant}`
  }

  function buildVariantMeta(input: {
    model: { providerID: string; modelID: string }
    variant?: string
    availableVariants: string[]
  }) {
    return {
      gizzi: {
        modelId: `${input.model.providerID}/${input.model.modelID}`,
        variant: input.variant ?? null,
        availableVariants: input.availableVariants,
      },
    }
  }

  function parseModelSelection(
    modelId: string,
    providers: Array<{ id: string; models: Record<string, { variants?: Record<string, any> }> }>,
  ): { model: { providerID: string; modelID: string }; variant?: string } {
    const parsed = Provider.parseModel(modelId)
    const provider = providers.find((p) => p.id === parsed.providerID)
    if (!provider) {
      return { model: parsed, variant: undefined }
    }

    // Check if modelID exists directly
    if (provider.models[parsed.modelID]) {
      return { model: parsed, variant: undefined }
    }

    // Try to extract variant from end of modelID (e.g., "claude-sonnet-5-5/high" -> model: "claude-sonnet-5-5", variant: "high")
    const segments = parsed.modelID.split("/")
    if (segments.length > 1) {
      const candidateVariant = segments[segments.length - 1]
      const baseModelId = segments.slice(0, -1).join("/")
      const baseModelInfo = provider.models[baseModelId]
      if (baseModelInfo?.variants && candidateVariant in baseModelInfo.variants) {
        return {
          model: { providerID: parsed.providerID, modelID: baseModelId },
          variant: candidateVariant,
        }
      }
    }

    return { model: parsed, variant: undefined }
  }
}
