import { Installation } from "@/shared/installation"
import { Provider } from "@/runtime/providers/provider"
import { Log } from "@/shared/util/log"
import {
  streamText,
  wrapLanguageModel,
  type ModelMessage,
  type StreamTextResult,
  type Tool,
  type ToolSet,
  tool,
  jsonSchema,
} from "ai"
import { mergeDeep, pipe } from "remeda"
import { ProviderTransform } from "@/runtime/providers/adapters/transform"
import { Config } from "@/runtime/context/config/config"
import { Instance } from "@/runtime/context/project/instance"
import type { Agent } from "@/runtime/loop/agent"
import type { MessageV2 } from "@/runtime/session/message-v2"
import { Plugin } from "@/runtime/integrations/plugin"
import { SystemPrompt } from "@/runtime/session/system"
import { Flag } from "@/runtime/context/flag/flag"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { runWithStreamContext } from "@/runtime/session/stream-context"
import { Auth } from "@/runtime/integrations/auth"
import { SessionTrace } from "@/runtime/session/trace"
import { ContextProjector } from "@/runtime/session/context-projector"
import { ContextAccounting } from "@/runtime/session/context-accounting"
import { RuntimeTelemetry } from "@/runtime/telemetry"
import { Bus } from "@/shared/bus"
import { SessionContext } from "./context-event"

export namespace LLM {
  const log = Log.create({ service: "llm" })
  export const OUTPUT_TOKEN_MAX = ProviderTransform.OUTPUT_TOKEN_MAX

  export type StreamInput = {
    user: MessageV2.User
    sessionID: string
    model: Provider.Model
    agent: Agent.Info
    system: string[]
    abort: AbortSignal
    messages: ModelMessage[]
    small?: boolean
    tools: Record<string, Tool>
    retries?: number
    toolChoice?: "auto" | "required" | "none"
    mode?: 'plan' | 'build'  // Execution mode for system prompt
    plan?: Provider.AuthPlan
  }

  export type StreamOutput = StreamTextResult<ToolSet, any>

  export async function stream(input: StreamInput) {
    const l = log
      .clone()
      .tag("providerID", input.model.providerID)
      .tag("modelID", input.model.id)
      .tag("sessionID", input.sessionID)
      .tag("small", (input.small ?? false).toString())
      .tag("agent", input.agent.name)
      .tag("mode", input.agent.mode)
    l.info("stream", {
      modelID: input.model.id,
      providerID: input.model.providerID,
    })
    const [language, cfg, provider, auth] = await Promise.all([
      Provider.getLanguage(input.model, input.plan),
      Config.get(),
      Provider.getProvider(input.model.providerID),
      Auth.get(input.model.providerID),
    ])
    const isCodex = provider.id === "openai" && auth?.type === "oauth"

    const system = []
    system.push(
      [
        // use agent prompt otherwise provider prompt
        // For Codex sessions, skip SystemPrompt.provider() since it's sent via options.instructions
        ...(input.agent.prompt ? [input.agent.prompt] : isCodex ? [] : SystemPrompt.provider(input.model, input.mode)),
        // any custom prompt passed into this call — already includes the last
        // user message's system override/append (see prompt.ts's loop(),
        // which folds lastUser.system into this array exactly once; do not
        // re-add input.user.system here or it gets applied twice).
        ...input.system,
      ]
        .filter((x) => x)
        .join("\n"),
    )

    const header = system[0]
    await Plugin.trigger(
      "experimental.chat.system.transform",
      { sessionID: input.sessionID, model: input.model },
      { system },
    )
    // rejoin to maintain 2-part structure for caching if header unchanged
    if (system.length > 2 && system[0] === header) {
      const rest = system.slice(1)
      system.length = 0
      system.push(header, rest.join("\n"))
    }

    const variant =
      !input.small && input.model.variants && input.user.variant ? input.model.variants[input.user.variant] : {}
    const base = input.small
      ? ProviderTransform.smallOptions(input.model)
      : ProviderTransform.options({
          model: input.model,
          sessionID: input.sessionID,
          providerOptions: provider.options,
        })
    let options: Record<string, any> = pipe(
      base,
      mergeDeep(input.model.options),
      mergeDeep(input.agent.options),
      mergeDeep(variant),
    )
    if (isCodex) {
      options.instructions = SystemPrompt.instructions()
    }
    // Forward service tier from API bridges (e.g. OpenAI `service_tier`) into
    // provider-specific options so the AI SDK can set the request body field.
    const serviceTier = input.user.metadata?.service_tier
    if (typeof serviceTier === "string") {
      options = mergeDeep(
        options,
        ProviderTransform.providerOptions(input.model, { serviceTier }),
      )
    }
    // Provider routing from API bridges (Allternit LLM gateway): the resolved
    // OpenRouter-style `provider` object is injected as a top-level body key.
    // Scoped to OpenAI-compatible providers (aggregators/OpenRouter); direct
    // SDKs (Anthropic, Google) must not receive it.
    const providerRouting = input.user.metadata?.provider_routing
    if (
      providerRouting !== null &&
      typeof providerRouting === "object" &&
      input.model.api.npm === "@ai-sdk/openai-compatible"
    ) {
      options = mergeDeep(
        options,
        ProviderTransform.providerOptions(input.model, { provider: providerRouting }),
      )
    }
    // BYO provider credentials from API bridges (Allternit LLM gateway): a
    // caller-supplied apiKey/baseURL rides the same per-turn metadata vehicle
    // as provider routing and overrides the provider's configured credentials
    // for this request only. SDK config options (not request body), so they
    // merge directly into `options`.
    const providerCredentials = input.user.metadata?.provider_routing_credentials
    if (
      providerCredentials !== null &&
      typeof providerCredentials === "object" &&
      input.model.api.npm === "@ai-sdk/openai-compatible"
    ) {
      const override: Record<string, any> = {}
      if (typeof providerCredentials["apiKey"] === "string") {
        override["apiKey"] = providerCredentials["apiKey"]
      }
      if (typeof providerCredentials["baseURL"] === "string") {
        override["baseURL"] = providerCredentials["baseURL"]
      }
      if (Object.keys(override).length > 0) {
        options = mergeDeep(options, override)
      }
    }

    const params = await Plugin.trigger(
      "chat.params",
      {
        sessionID: input.sessionID,
        agent: input.agent,
        model: input.model,
        provider,
        message: input.user,
      },
      {
        temperature: input.model.capabilities.temperature
          ? (input.agent.temperature ?? ProviderTransform.temperature(input.model))
          : undefined,
        topP: input.agent.topP ?? ProviderTransform.topP(input.model),
        topK: ProviderTransform.topK(input.model),
        options,
      },
    )

    const { headers } = await Plugin.trigger(
      "chat.headers",
      {
        sessionID: input.sessionID,
        agent: input.agent,
        model: input.model,
        provider,
        message: input.user,
      },
      {
        headers: {},
      },
    )

    const maxOutputTokens =
      isCodex ? undefined : ProviderTransform.maxOutputTokens(input.model)

    const tools = await resolveTools(input)

    // LiteLLM and some Anthropic proxies require the tools parameter to be present
    // when message history contains tool calls, even if no tools are being used.
    // Add a dummy tool that is never called to satisfy this validation.
    // This is enabled for:
    // 1. Providers with "litellm" in their ID or API ID (auto-detected)
    // 2. Providers with explicit "litellmProxy: true" option (opt-in for custom gateways)
    const isLiteLLMProxy =
      provider.options?.["litellmProxy"] === true ||
      input.model.providerID.toLowerCase().includes("litellm") ||
      input.model.api.id.toLowerCase().includes("litellm")

    if (isLiteLLMProxy && Object.keys(tools).length === 0 && hasToolCalls(input.messages)) {
      tools["_noop"] = tool({
        description:
          "Placeholder for LiteLLM/Anthropic proxy compatibility - required when message history contains tool calls but no active tools are needed",
        inputSchema: jsonSchema({ type: "object", properties: {} }),
        execute: async () => ({ output: "", title: "", metadata: {} }),
      })
    }

    const projected = Flag.GIZZI_DISABLE_CONTEXT_PROJECTION
      ? { messages: input.messages, anomalies: [] }
      : ContextProjector.project(input.messages)
    if (projected.anomalies.length) {
      const counts = (kind: string) => projected.anomalies.filter((item) => item.kind === kind).length
      RuntimeTelemetry.track("context_projection_repaired", {
        reordered: counts("tool_result_reordered"),
        synthesized: counts("tool_result_synthesized"),
        dropped: projected.anomalies.filter((item) => item.kind.includes("dropped")).length,
        merged: counts("consecutive_assistants_merged"),
      })
    }
    const context = ContextAccounting.measure({
      messages: projected.messages,
      system,
      tools,
      contextWindow: input.model.limit.context,
      reservedOutputTokens: maxOutputTokens ?? Math.min(32_000, input.model.limit.output),
    })
    Bus.publish(SessionContext.Event.Updated, {
      sessionID: input.sessionID,
      used: context.inputTokens,
      window: context.contextWindow || undefined,
      basis: context.basis,
    })
    SessionTrace.append({
      sessionID: input.sessionID,
      kind: "request.started",
      messageID: input.user.id,
      data: {
        requestID: input.user.id,
        providerID: input.model.providerID,
        modelID: input.model.id,
        agent: input.agent.name,
        toolNames: Object.keys(tools).filter((name) => name !== "invalid"),
        mcpToolNames: Object.keys(tools).filter((name) => name.includes(":")),
        messageCount: input.messages.length,
        projectedMessageCount: projected.messages.length,
        projectionRepairs: projected.anomalies,
        systemBlockCount: system.length,
        context,
        params: redactParams({
          temperature: params.temperature,
          topP: params.topP,
          topK: params.topK,
          maxOutputTokens,
          toolChoice: input.toolChoice,
          providerOptions: ProviderTransform.providerOptions(input.model, params.options),
        }),
      },
    })

    return runWithStreamContext({ sessionID: input.sessionID }, () =>
      streamText({
      // Raw model chunks are the sanctioned AI SDK sideband for
      // provider-internal events (see `includeRawChunks` below): the
      // subprocess adapter forwards CLI-observed tool invocations as raw
      // parts and the processor turns them into session tool parts.
      includeRawChunks: true,
      onError(error) {
        l.error("stream error", {
          error,
        })
      },
      async experimental_repairToolCall(failed) {
        const lower = failed.toolCall.toolName.toLowerCase()
        if (lower !== failed.toolCall.toolName && tools[lower]) {
          l.info("repairing tool call", {
            tool: failed.toolCall.toolName,
            repaired: lower,
          })
          return {
            ...failed.toolCall,
            toolName: lower,
          }
        }
        return {
          ...failed.toolCall,
          input: JSON.stringify({
            tool: failed.toolCall.toolName,
            error: failed.error.message,
          }),
          toolName: "invalid",
        }
      },
      temperature: params.temperature,
      topP: params.topP,
      topK: params.topK,
      providerOptions: ProviderTransform.providerOptions(input.model, params.options),
      activeTools: Object.keys(tools).filter((x) => x !== "invalid"),
      tools,
      toolChoice: input.toolChoice,
      maxOutputTokens,
      abortSignal: input.abort,
      headers: {
        // Always present so subprocess/ACP drivers can resolve the session
        // even if AsyncLocalStorage is not on the doStream call stack.
        "x-gizzi-session": input.sessionID,
        // Subscription Fabric: this send's human action (D16), minted by the
        // chat bridge, and the request id that keys the task's idempotency.
        ...(Provider.isFabricModel(input.model)
          ? {
              "x-gizzi-request": input.user.id,
              ...(typeof (input.user.metadata?.subscription as any)?.action_id === "string"
                ? { "x-allternit-human-action": (input.user.metadata?.subscription as any).action_id }
                : {}),
            }
          : {}),
        ...(input.model.providerID.startsWith("gizzi")
          ? {
              "x-gizzi-project": Instance.project.id,
              "x-gizzi-request": input.user.id,
              "x-gizzi-client": Flag.GIZZI_CLIENT,
            }
          : input.model.providerID !== "anthropic"
            ? {
                "User-Agent": `gizzi/${Installation.VERSION}`,
              }
            : undefined),
        ...input.model.headers,
        ...headers,
      },
      maxRetries: input.retries ?? 0,
      messages: [
        ...system.map(
          (x): ModelMessage => ({
            role: "system",
            content: x,
          }),
        ),
        ...projected.messages,
      ],
      model: wrapLanguageModel({
        model: language as any,
        middleware: [
          {
            specificationVersion: "v3" as const,
            async transformParams(args: { type: string; params: any }) {
              if (args.type === "stream") {
                args.params.prompt = ProviderTransform.message(args.params.prompt, input.model, options)
              }
              return args.params
            },
          } as any,
        ],
      }),
      experimental_telemetry: {
        isEnabled: cfg.experimental?.openTelemetry,
        metadata: {
          userId: cfg.username ?? "unknown",
          sessionId: input.sessionID,
        },
      },
      })
    )
  }

  async function resolveTools(input: Pick<StreamInput, "tools" | "agent" | "user">) {
    const disabled = PermissionNext.disabled(Object.keys(input.tools), input.agent.permission)
    for (const tool of Object.keys(input.tools)) {
      if (input.user.tools?.[tool] === false || disabled.has(tool)) {
        delete input.tools[tool]
      }
    }
    return input.tools
  }

  // Check if messages contain any tool-call content
  // Used to determine if a dummy tool should be added for LiteLLM proxy compatibility
  export function hasToolCalls(messages: ModelMessage[]): boolean {
    for (const msg of messages) {
      if (!Array.isArray(msg.content)) continue
      for (const part of msg.content) {
        if (part.type === "tool-call" || part.type === "tool-result") return true
      }
    }
    return false
  }

  function redactParams(input: unknown, key = ""): unknown {
    if (/(?:key|authorization|password|secret|token|cookie|credential)/i.test(key)) return "<REDACTED>"
    if (Array.isArray(input)) return input.map((value) => redactParams(value))
    if (input && typeof input === "object") {
      return Object.fromEntries(Object.entries(input).map(([name, value]) => [name, redactParams(value, name)]))
    }
    return input
  }
}
