import z from "zod/v4"
import fuzzysort from "fuzzysort"
import { Config } from "@/runtime/context/config/config"
import { mapValues, mergeDeep, omit, pickBy, sortBy } from "remeda"
import { Log } from "@/shared/util/log"
import { BunProc } from "@/shared/bun"
import { Plugin } from "@/runtime/integrations/plugin"
import { ModelsDev } from "@/runtime/providers/adapters/models"
import { NamedError } from "@allternit/gizzi-util/error.js"
import { Auth } from "@/runtime/integrations/auth"
import { Env } from "@/runtime/context/env"
import { Instance } from "@/runtime/context/project/instance"
import { Flag } from "@/runtime/context/flag/flag"
import { iife } from "@/shared/util/iife"
import { GlobalPaths } from "@/runtime/context/global/paths"
import path from "path"
import { Filesystem } from "@/shared/util/filesystem"
import { Sidecar } from "@/runtime/sidecar"
import { ProviderTransform } from "@/runtime/providers/adapters/transform"

// Confined AI SDK imports — all provider SDK access goes through adapters/bundled.ts
import { BUNDLED_PROVIDERS, NoSuchModelError, type SDK, type LanguageModelV2 } from "@/runtime/providers/adapters/bundled"

// Provider-specific loaders — each provider's config/auth/model logic in its own file
import { CUSTOM_LOADERS } from "@/runtime/providers/adapters/loaders"
import type { CustomModelLoader } from "@/runtime/providers/types"
import { Discovery } from "@/runtime/providers/discovery"
import { SubprocessLanguageModel } from "@/runtime/providers/adapters/loaders/subprocess"
import { SubscriptionFabricLanguageModel } from "@/runtime/providers/fabric/language-model"
import { isFabricProviderID } from "@/runtime/providers/fabric/client"
import { cliModel, stripProviderPrefix } from "@/runtime/providers/cli-model"
import { tapRetryHint } from "@/runtime/providers/retry-hint"

export namespace Provider {
  const log = Log.create({ service: "provider" })

  // TODO(types): the plugin SDK's declared AuthHook.loader is narrower than
  // what auth plugins provide at runtime (2-arg loader returning
  // apiKey/authType); see src/runtime/integrations/plugin/codex.ts and
  // src/runtime/providers/adapters/auth.ts for the same mismatch.
  type RuntimeAuthLoader = (
    getAuth: () => Promise<unknown>,
    provider: unknown,
  ) => Promise<{ apiKey?: string; token?: string; authType?: string } | undefined>

  // Per-provider HTTP concurrency cap. Configured via provider.options.concurrency
  // (max simultaneous requests). Unset or <= 0 means uncapped (default behavior).
  class Semaphore {
    private active = 0
    private queue: Array<() => void> = []
    constructor(private readonly max: number) {}
    async acquire(): Promise<void> {
      if (this.active < this.max) {
        this.active++
        return
      }
      await new Promise<void>((resolve) =>
        this.queue.push(() => {
          this.active++
          resolve()
        }),
      )
    }
    release() {
      this.active = Math.max(0, this.active - 1)
      const next = this.queue.shift()
      if (next) next()
    }
  }

  const providerSemaphores = new Map<string, Semaphore>()

  function providerSemaphore(providerID: string, max: number): Semaphore {
    let sem = providerSemaphores.get(providerID)
    if (!sem) {
      sem = new Semaphore(max)
      providerSemaphores.set(providerID, sem)
    }
    return sem
  }

  export function loadBaseURL(model: Model, options: Record<string, any>) {
    const raw = options["baseURL"] ?? model.api.url
    if (typeof raw !== "string") return raw
    return raw.replace(/\$\{([^}]+)\}/g, (match, key) => {
      const val = Env.get(String(key))
      return val ?? match
    })
  }

  // CUSTOM_LOADERS and BUNDLED_PROVIDERS are imported from adapters/

  export const Model = z
    .object({
      id: z.string(),
      providerID: z.string(),
      api: z.object({
        id: z.string(),
        url: z.string(),
        npm: z.string(),
      }),
      name: z.string(),
      family: z.string().optional(),
      capabilities: z.object({
        temperature: z.boolean(),
        reasoning: z.boolean(),
        attachment: z.boolean(),
        toolcall: z.boolean(),
        input: z.object({
          text: z.boolean(),
          audio: z.boolean(),
          image: z.boolean(),
          video: z.boolean(),
          pdf: z.boolean(),
        }),
        output: z.object({
          text: z.boolean(),
          audio: z.boolean(),
          image: z.boolean(),
          video: z.boolean(),
          pdf: z.boolean(),
        }),
        interleaved: z.union([
          z.boolean(),
          z.object({
            field: z.enum(["reasoning_content", "reasoning_details"]),
          }),
        ]),
      }),
      cost: z.object({
        input: z.number(),
        output: z.number(),
        cache: z.object({
          read: z.number(),
          write: z.number(),
        }),
        experimentalOver200K: z
          .object({
            input: z.number(),
            output: z.number(),
            cache: z.object({
              read: z.number(),
              write: z.number(),
            }),
          })
          .optional(),
      }),
      limit: z.object({
        context: z.number(),
        input: z.number().optional(),
        output: z.number(),
      }),
      status: z.enum(["alpha", "beta", "deprecated", "active"]),
      options: z.record(z.string(), z.any()),
      headers: z.record(z.string(), z.string()),
      release_date: z.string(),
      variants: z.record(z.string(), z.record(z.string(), z.any())).optional(),
    })
    
  export type Model = z.infer<typeof Model>

  export const Info = z
    .object({
      id: z.string(),
      name: z.string(),
      source: z.enum(["env", "config", "custom", "api"]),
      env: z.string().array(),
      key: z.string().optional(),
      // Auth type controls how llm_config is forwarded to the operator:
      //   "api_key"    — standard secret key (default)
      //   "none"       — local model, no auth (Ollama, LM Studio, vLLM)
      //   "bearer"     — OAuth / subscription token in Authorization: Bearer
      //   "subprocess" — CLI tool already authed in OS (claude, llm, aichat)
      auth_type: z.enum(["api_key", "none", "bearer", "subprocess"]).optional(),
      token: z.string().optional(),          // bearer auth token
      subprocess_cmd: z.string().optional(), // e.g. "claude -p" or "/usr/local/bin/llm -m gpt-4o"
      options: z.record(z.string(), z.any()),
      models: z.record(z.string(), Model),
    })

  export type Info = z.infer<typeof Info>

  export type ModelRef = {
    providerID: string
    modelID: string
    authProfileId?: string
  }

  export type AuthPlan = {
    providerID: string
    modelID: string
    baseURL: string
    apiKey?: string
    token?: string
    authType: "api_key" | "none" | "bearer" | "subprocess"
    extraHeaders: Record<string, string>
    source: "profile" | "config" | "env" | "plugin" | "subprocess" | "none"
    profileId?: string
  }

  export type RuntimePolicy = "sdk" | "subprocess" | "auto"

  function fromModelsDevModel(provider: ModelsDev.Provider, model: ModelsDev.Model): Model {
    const m: Model = {
      id: model.id,
      providerID: provider.id,
      name: model.name,
      family: model.family,
      api: {
        id: model.id,
        url: model.provider?.api ?? provider.api!,
        npm: model.provider?.npm ?? provider.npm ?? "@ai-sdk/openai-compatible",
      },
      status: model.status ?? "active",
      headers: model.headers ?? {},
      options: model.options ?? {},
      cost: {
        input: model.cost?.input ?? 0,
        output: model.cost?.output ?? 0,
        cache: {
          read: model.cost?.cache_read ?? 0,
          write: model.cost?.cache_write ?? 0,
        },
        experimentalOver200K: model.cost?.context_over_200k
          ? {
              cache: {
                read: model.cost.context_over_200k.cache_read ?? 0,
                write: model.cost.context_over_200k.cache_write ?? 0,
              },
              input: model.cost.context_over_200k.input,
              output: model.cost.context_over_200k.output,
            }
          : undefined,
      },
      limit: {
        context: model.limit.context,
        input: model.limit.input,
        output: model.limit.output,
      },
      capabilities: {
        temperature: model.temperature,
        reasoning: model.reasoning,
        attachment: model.attachment,
        toolcall: model.tool_call,
        input: {
          text: model.modalities?.input?.includes("text") ?? false,
          audio: model.modalities?.input?.includes("audio") ?? false,
          image: model.modalities?.input?.includes("image") ?? false,
          video: model.modalities?.input?.includes("video") ?? false,
          pdf: model.modalities?.input?.includes("pdf") ?? false,
        },
        output: {
          text: model.modalities?.output?.includes("text") ?? false,
          audio: model.modalities?.output?.includes("audio") ?? false,
          image: model.modalities?.output?.includes("image") ?? false,
          video: model.modalities?.output?.includes("video") ?? false,
          pdf: model.modalities?.output?.includes("pdf") ?? false,
        },
        interleaved: model.interleaved ?? false,
      },
      release_date: model.release_date,
      variants: {},
    }

    m.variants = mapValues(ProviderTransform.variants(m), (v) => v)

    return m
  }

  export function fromModelsDevProvider(provider: ModelsDev.Provider): Info {
    return {
      id: provider.id,
      source: "custom",
      name: provider.name,
      env: provider.env ?? [],
      options: {},
      models: mapValues(provider.models, (model) => fromModelsDevModel(provider, model)),
    }
  }

  const state = Instance.state(async () => {
    using _ = log.time("state")
    const config = await Config.get()
    const modelsDev = await ModelsDev.get()
    const database = mapValues(modelsDev, fromModelsDevProvider)

    const disabled = new Set(config.disabled_providers ?? [])
    const enabled = config.enabled_providers ? new Set(config.enabled_providers) : null

    function isProviderAllowed(providerID: string): boolean {
      if (providerID === "vercel") return false
      if (enabled && !enabled.has(providerID)) return false
      if (disabled.has(providerID)) return false
      return true
    }

    const providers: { [providerID: string]: Info } = {}
    const languages = new Map<string, LanguageModelV2>()
    const modelLoaders: {
      [providerID: string]: CustomModelLoader
    } = {}
    const sdk = new Map<number, SDK>()

    log.info("init")

    const configProviders = Object.entries(config.provider ?? {})

    function mergeProvider(providerID: string, provider: Partial<Info>) {
      const existing = providers[providerID]
      if (existing) {
        providers[providerID] = mergeDeep(existing, provider) as Info
        return
      }
      const match = database[providerID]
      if (!match) return
      providers[providerID] = mergeDeep(match, provider) as Info
    }

    // extend database from config
    for (const [providerID, provider] of configProviders) {
      const existing = database[providerID]
      const parsed: Info = {
        id: providerID,
        name: provider.name ?? existing?.name ?? providerID,
        env: provider.env ?? existing?.env ?? [],
        options: mergeDeep(existing?.options ?? {}, provider.options ?? {}),
        source: "config",
        models: existing?.models ?? {},
      }

      for (const [modelID, model] of Object.entries(provider.models ?? {})) {
        const existingModel = parsed.models[model.id ?? modelID]
        const name = iife(() => {
          if (model.name) return model.name
          if (model.id && model.id !== modelID) return modelID
          return existingModel?.name ?? modelID
        })
        const parsedModel: Model = {
          id: modelID,
          api: {
            id: model.id ?? existingModel?.api.id ?? modelID,
            npm:
              model.provider?.npm ??
              provider.npm ??
              existingModel?.api.npm ??
              modelsDev[providerID]?.npm ??
              "@ai-sdk/openai-compatible",
            url: model.provider?.api ?? provider?.api ?? provider?.options?.["baseURL"] ?? existingModel?.api.url ?? modelsDev[providerID]?.api,
          },
          status: model.status ?? existingModel?.status ?? "active",
          name,
          providerID,
          capabilities: {
            temperature: model.temperature ?? existingModel?.capabilities.temperature ?? false,
            reasoning: model.reasoning ?? existingModel?.capabilities.reasoning ?? false,
            attachment: model.attachment ?? existingModel?.capabilities.attachment ?? false,
            toolcall: model.tool_call ?? existingModel?.capabilities.toolcall ?? true,
            input: {
              text: model.modalities?.input?.includes("text") ?? existingModel?.capabilities.input.text ?? true,
              audio: model.modalities?.input?.includes("audio") ?? existingModel?.capabilities.input.audio ?? false,
              image: model.modalities?.input?.includes("image") ?? existingModel?.capabilities.input.image ?? false,
              video: model.modalities?.input?.includes("video") ?? existingModel?.capabilities.input.video ?? false,
              pdf: model.modalities?.input?.includes("pdf") ?? existingModel?.capabilities.input.pdf ?? false,
            },
            output: {
              text: model.modalities?.output?.includes("text") ?? existingModel?.capabilities.output.text ?? true,
              audio: model.modalities?.output?.includes("audio") ?? existingModel?.capabilities.output.audio ?? false,
              image: model.modalities?.output?.includes("image") ?? existingModel?.capabilities.output.image ?? false,
              video: model.modalities?.output?.includes("video") ?? existingModel?.capabilities.output.video ?? false,
              pdf: model.modalities?.output?.includes("pdf") ?? existingModel?.capabilities.output.pdf ?? false,
            },
            interleaved: model.interleaved ?? false,
          },
          cost: {
            input: model?.cost?.input ?? existingModel?.cost?.input ?? 0,
            output: model?.cost?.output ?? existingModel?.cost?.output ?? 0,
            cache: {
              read: model?.cost?.cache_read ?? existingModel?.cost?.cache.read ?? 0,
              write: model?.cost?.cache_write ?? existingModel?.cost?.cache.write ?? 0,
            },
          },
          options: mergeDeep(existingModel?.options ?? {}, model.options ?? {}),
          limit: {
            context: model.limit?.context ?? existingModel?.limit?.context ?? 0,
            output: model.limit?.output ?? existingModel?.limit?.output ?? 0,
          },
          headers: mergeDeep(existingModel?.headers ?? {}, model.headers ?? {}),
          family: model.family ?? existingModel?.family ?? "",
          release_date: model.release_date ?? existingModel?.release_date ?? "",
          variants: {},
        }
        const merged: Record<string, Record<string, any>> = mergeDeep(ProviderTransform.variants(parsedModel), model.variants ?? {})
        parsedModel.variants = mapValues(
          pickBy(merged, (v) => !v.disabled),
          (v) => omit(v, ["disabled"]),
        )
        parsed.models[modelID] = parsedModel
      }
      database[providerID] = parsed
    }

    // load env
    const env = Env.all()
    for (const [providerID, provider] of Object.entries(database)) {
      if (disabled.has(providerID)) continue
      const apiKey = provider.env.map((item) => env[item]).find(Boolean)
      if (!apiKey) continue
      mergeProvider(providerID, {
        source: "env",
        key: provider.env.length === 1 ? apiKey : undefined,
      })
    }

    // load apikeys
    for (const [providerID, provider] of Object.entries(await Auth.all())) {
      if (disabled.has(providerID)) continue
      if (provider.type === "api") {
        mergeProvider(providerID, {
          source: "api",
          key: provider.key,
        })
      }
    }

    for (const plugin of await Plugin.list()) {
      if (!plugin.auth) continue
      const providerID = plugin.auth.provider
      if (disabled.has(providerID)) continue

      const auth = await Auth.get(providerID)
      if (!auth) continue
      if (!plugin.auth.loader) continue

      const options = await (plugin.auth.loader as unknown as RuntimeAuthLoader)(
        () => Auth.get(providerID) as any,
        database[plugin.auth.provider],
      )
      const opts = options ?? {}
      const patch: Partial<Info> = providers[providerID] ? { options: opts } : { source: "custom", options: opts }
      mergeProvider(providerID, patch)
    }

    for (const [providerID, fn] of Object.entries(CUSTOM_LOADERS)) {
      if (disabled.has(providerID)) continue
      const data = database[providerID]
      if (!data) {
        log.info("custom loader skipped; provider not in model database " + providerID)
        continue
      }
      const result = await fn(data)
      if (result && (result.autoload || providers[providerID])) {
        if (result.getModel) modelLoaders[providerID] = result.getModel
        const opts = result.options ?? {}
        const patch: Partial<Info> = providers[providerID] ? { options: opts } : { source: "custom", options: opts }
        mergeProvider(providerID, patch)
      }
    }

    // load config
    for (const [providerID, provider] of configProviders) {
      const partial: Partial<Info> = { source: "config" }
      if (provider.env) partial.env = provider.env
      if (provider.name) partial.name = provider.name
      if (provider.options) partial.options = provider.options
      if (provider.auth_type) partial.auth_type = provider.auth_type
      if (provider.token) partial.token = provider.token
      if (provider.subprocess_cmd) partial.subprocess_cmd = provider.subprocess_cmd
      mergeProvider(providerID, partial)
    }

    // Inject sidecar embedded model as a provider (if sidecar is available)
    if (!providers["sidecar"]) {
      const sidecarConfig = await Sidecar.providerConfig()
      if (sidecarConfig) {
        const sidecarModels: Record<string, Model> = {}
        for (const [modelID, model] of Object.entries(sidecarConfig.models)) {
          const m = model as any
          sidecarModels[modelID] = {
            id: modelID,
            api: { id: m.id ?? modelID, npm: sidecarConfig.npm, url: Sidecar.BaseURL },
            name: m.name ?? modelID,
            providerID: "sidecar",
            family: "qwen",
            status: "active",
            capabilities: {
              temperature: false,
              reasoning: false,
              attachment: false,
              toolcall: m.tool_call ?? true,
              input: { text: true, audio: false, image: false, video: false, pdf: false },
              output: { text: true, audio: false, image: false, video: false, pdf: false },
              interleaved: false,
            },
            cost: { input: 0, output: 0, cache: { read: 0, write: 0 } },
            limit: { context: m.limit?.context ?? 32768, output: m.limit?.output ?? 4096 },
            options: {},
            headers: {},
            release_date: "2025-01-01",
          }
        }
        providers["sidecar"] = {
          id: "sidecar",
          name: "Embedded Sidecar",
          env: [],
          options: sidecarConfig.options as any,
          source: "config",
          models: sidecarModels,
        }
        log.info("registered sidecar provider", { models: Object.keys(sidecarModels) })
      }
    }

    // ── Auto-discovery: subprocess CLIs + local HTTP servers ──────────────
    // Runs in parallel with provider loading. Discovered providers are injected
    // here with lower priority — user config and env always win.
    const discovered = await Discovery.run()
    for (const dp of discovered) {
      if (!isProviderAllowed(dp.id)) continue
      const dpModels: Record<string, Model> = {}
      for (const m of dp.models) {
        dpModels[m.id] = {
          id: m.id,
          providerID: dp.id,
          name: m.name,
          family: "",
          api: {
            id: m.id,
            url: dp.base_url ?? "",
            npm: "@ai-sdk/openai-compatible",
          },
          status: "active",
          headers: {},
          options: {},
          release_date: "",
          capabilities: {
            temperature: dp.options?.["runtime"] !== "fabric",
            reasoning: false,
            attachment: false,
            // Fabric chat runs in the provider's web UI: gizzi's tools cannot be
            // called there. Also keeps these models out of auto-tier picks.
            toolcall: dp.options?.["runtime"] !== "fabric",
            input:  { text: true, audio: false, image: false, video: false, pdf: false },
            output: { text: true, audio: false, image: false, video: false, pdf: false },
            interleaved: false,
          },
          cost: { input: 0, output: 0, cache: { read: 0, write: 0 } },
          limit: { context: m.context ?? 32768, output: m.output ?? 4096 },
          variants: {},
        }
      }
      if (Object.keys(dpModels).length === 0) continue
      const existing = providers[dp.id]
      if (existing) {
        for (const [modelID, model] of Object.entries(dpModels)) {
          if (!existing.models[modelID]) existing.models[modelID] = model
        }
        if (dp.subprocess_cmd && !existing.subprocess_cmd) existing.subprocess_cmd = dp.subprocess_cmd
        if (dp.auth_type && !existing.auth_type) existing.auth_type = dp.auth_type
        log.info("discovered-merged", { providerID: dp.id, source: dp.source, models: Object.keys(dpModels).length })
        continue
      }
      providers[dp.id] = {
        id: dp.id,
        name: dp.name,
        source: "custom",
        env: dp.id === "allternit" ? ["ALLTERNIT_API_KEY"] : [],
        key: dp.id === "allternit" ? process.env.ALLTERNIT_API_KEY : undefined,
        auth_type: dp.auth_type,
        subprocess_cmd: dp.subprocess_cmd,
        options: { ...(dp.base_url ? { baseURL: dp.base_url } : {}), ...(dp.options ?? {}) },
        models: dpModels,
      }
      log.info("discovered", { providerID: dp.id, source: dp.source, models: Object.keys(dpModels).length })
    }

    for (const [providerID, provider] of Object.entries(providers)) {
      if (!isProviderAllowed(providerID)) {
        delete providers[providerID]
        continue
      }

      const configProvider = config.provider?.[providerID]

      for (const [modelID, model] of Object.entries(provider.models)) {
        model.api.id = model.api.id ?? model.id ?? modelID
        if (modelID === "gpt-5-chat-latest") delete provider.models[modelID]
        if (model.status === "alpha" && !Flag.GIZZI_ENABLE_EXPERIMENTAL_MODELS) delete provider.models[modelID]
        if (model.status === "deprecated") delete provider.models[modelID]
        if (
          (configProvider?.blacklist && configProvider.blacklist.includes(modelID)) ||
          (configProvider?.whitelist && !configProvider.whitelist.includes(modelID))
        )
          delete provider.models[modelID]

        model.variants = mapValues(ProviderTransform.variants(model), (v) => v)

        // Filter out disabled variants from config
        const configVariants = configProvider?.models?.[modelID]?.variants
        if (configVariants && model.variants) {
          const merged: Record<string, Record<string, any>> = mergeDeep(model.variants, configVariants)
          model.variants = mapValues(
            pickBy(merged, (v) => !v.disabled),
            (v) => omit(v, ["disabled"]),
          )
        }
      }

      if (Object.keys(provider.models).length === 0) {
        delete providers[providerID]
        continue
      }

      log.info("found", { providerID })
    }

    return {
      models: languages,
      providers,
      sdk,
      modelLoaders,
    }
  })

  export async function list() {
    return state().then((state) => state.providers)
  }

  async function getSDK(model: Model, plan?: AuthPlan) {
    try {
      using _ = log.time("getSDK", {
        providerID: model.providerID,
      })
      const s = await state()
      const provider = s.providers[model.providerID]
      const options = { ...provider?.options }

      if (model.api.npm.includes("@ai-sdk/openai-compatible") && options["includeUsage"] !== false) {
        options["includeUsage"] = true
      }

      if (plan) {
        if (plan.baseURL !== undefined) options["baseURL"] = plan.baseURL
        if (plan.apiKey !== undefined) options["apiKey"] = plan.apiKey
        if (plan.token !== undefined) {
          options["token"] = plan.token
          options["headers"] = {
            ...options["headers"],
            Authorization: `Bearer ${plan.token}`,
          }
        }
        options["headers"] = {
          ...options["headers"],
          ...plan.extraHeaders,
        }
      } else {
        const baseURL = loadBaseURL(model, options)
        if (baseURL !== undefined) options["baseURL"] = baseURL
        if (options["apiKey"] === undefined && provider?.key) options["apiKey"] = provider.key
      }
      if (model.headers)
        options["headers"] = {
          ...options["headers"],
          ...model.headers,
        }

      const key = Bun.hash.xxHash32(JSON.stringify({ providerID: model.providerID, npm: model.api.npm, options }))
      const existing = s.sdk.get(key)
      if (existing) return existing

      const customFetch = options["fetch"]

      options["fetch"] = async (input: any, init?: BunFetchRequestInit) => {
        // Preserve custom fetch if it exists, wrap it with timeout logic
        const fetchFn = customFetch ?? fetch
        const opts = init ?? {}

        if (options["timeout"] !== undefined && options["timeout"] !== null) {
          const signals: AbortSignal[] = []
          if (opts.signal) signals.push(opts.signal)
          if (options["timeout"] !== false) signals.push(AbortSignal.timeout(options["timeout"]))

          const combined = signals.length > 1 ? AbortSignal.any(signals) : signals[0]

          opts.signal = combined
        }

        // Strip openai itemId metadata following what codex does
        if (model.api.npm === "@ai-sdk/openai" && opts.body && opts.method === "POST") {
          const body = JSON.parse(opts.body as string)
          if (Array.isArray(body.input)) {
            for (const item of body.input) {
              if ("id" in item) {
                delete item.id
              }
            }
            opts.body = JSON.stringify(body)
          }
        }

        const reqInit = {
          ...opts,
          // @ts-ignore see here: https://github.com/oven-sh/bun/issues/16682
          timeout: false,
        }

        // Gateway streaming failover (allternit.retry_hint, gateway PR #728):
        // tap SSE chat-completions responses so the hint frame is stripped
        // before the AI SDK sees it and recorded for the session processor.
        const retryHintSession = iife(() => {
          try {
            return new Headers(opts.headers as HeadersInit | undefined).get("x-gizzi-session")
          } catch {
            return null
          }
        })
        const retryHintUrl = typeof input === "string" ? input : ((input as { url?: string })?.url ?? "")
        const wrapRetryHint = (res: Response) =>
          tapRetryHint({
            response: res,
            method: opts.method,
            url: retryHintUrl,
            sessionID: retryHintSession,
            npm: model.api.npm,
          })

        // Optional per-provider concurrency cap (provider.options.concurrency).
        const cap = Number(options["concurrency"])
        if (Number.isFinite(cap) && cap > 0) {
          const sem = providerSemaphore(model.providerID, Math.floor(cap))
          await sem.acquire()
          try {
            return wrapRetryHint(await fetchFn(input, reqInit))
          } finally {
            sem.release()
          }
        }

        return wrapRetryHint(await fetchFn(input, reqInit))
      }

      const bundledFn = BUNDLED_PROVIDERS[model.api.npm]
      if (bundledFn) {
        log.info("using bundled provider", { providerID: model.providerID, pkg: model.api.npm })
        const loaded = bundledFn({
          name: model.providerID,
          ...options,
        })
        s.sdk.set(key, loaded)
        return loaded as SDK
      }

      let installedPath: string
      if (!model.api.npm.startsWith("file://")) {
        installedPath = await BunProc.install(model.api.npm, "latest")
      } else {
        log.info("loading local provider", { pkg: model.api.npm })
        installedPath = model.api.npm
      }

      const mod = await import(installedPath)

      const fn = mod[Object.keys(mod).find((key) => key.startsWith("create"))!]
      const loaded = fn({
        name: model.providerID,
        ...options,
      })
      s.sdk.set(key, loaded)
      return loaded as SDK
    } catch (e) {
      throw new InitError({ providerID: model.providerID }, { cause: e })
    }
  }

  export async function getProvider(providerID: string) {
    return state().then((s) => s.providers[providerID])
  }

  export async function getModel(providerID: string, modelID: string) {
    modelID = stripProviderPrefix(providerID, modelID)
    const s = await state()
    const provider = s.providers[providerID]
    if (!provider) {
      const availableProviders = Object.keys(s.providers)
      const matches = fuzzysort.go(providerID, availableProviders, { limit: 3, threshold: -10000 })
      const suggestions = matches.map((m) => m.target)
      throw new ModelNotFoundError({ providerID, modelID, suggestions })
    }

    const info = provider.models[modelID] ?? cliModel(provider, modelID, log)
    if (!info) {
      const availableModels = Object.keys(provider.models)
      const matches = fuzzysort.go(modelID, availableModels, { limit: 3, threshold: -10000 })
      const suggestions = matches.map((m) => m.target)
      throw new ModelNotFoundError({ providerID, modelID, suggestions })
    }
    return info
  }

  export async function getLanguage(model: Model, plan?: AuthPlan): Promise<LanguageModelV2> {
    const s = await state()
    const key = `${model.providerID}/${model.id}`
    if (s.models.has(key)) return s.models.get(key)!

    const provider = s.providers[model.providerID]

    // Subscription Fabric providers (subs-*) — the user's web subscription on
    // their Sessions computer, through allternit-api's forwarder.
    if (provider?.options?.["runtime"] === "fabric") {
      const fabricProvider = String(provider.options["fabricProvider"] ?? model.providerID.replace(/^subs-/, ""))
      const language = new SubscriptionFabricLanguageModel(
        model.providerID,
        fabricProvider,
        model.api.id,
      ) as unknown as LanguageModelV2
      s.models.set(key, language)
      return language
    }

    // Subprocess providers (claude-cli, llm, aichat, etc.) — bypass HTTP SDK entirely
    const policy = await resolveRuntimePolicy({ providerID: model.providerID, modelID: model.id })
    if (policy === "subprocess" || plan?.authType === "subprocess") {
      const language = new SubprocessLanguageModel(model.providerID, model.api.id) as unknown as LanguageModelV2
      s.models.set(key, language)
      return language
    }

    const sdk = await getSDK(model, plan)

    try {
      const useCustomLoader =
        !!s.modelLoaders[model.providerID] &&
        !String(model.api.npm || "").includes("openai-compatible")
      const language = useCustomLoader
        ? await s.modelLoaders[model.providerID](sdk, model.api.id, provider?.options ?? {})
        : sdk.languageModel(model.api.id)
      s.models.set(key, language)
      return language
    } catch (e) {
      if (e instanceof NoSuchModelError)
        throw new ModelNotFoundError(
          {
            modelID: model.id,
            providerID: model.providerID,
          },
          { cause: e },
        )
      throw e
    }
  }

  export async function prepareAuth(ref: ModelRef): Promise<AuthPlan> {
    const [provider, model, cfg] = await Promise.all([
      getProvider(ref.providerID),
      getModel(ref.providerID, ref.modelID).catch(() => undefined),
      Config.get(),
    ])
    const configProvider = cfg.provider?.[ref.providerID]
    const options = { ...provider?.options, ...(configProvider?.options ?? {}) }

    const plan: Partial<AuthPlan> = {
      providerID: ref.providerID,
      modelID: ref.modelID,
      authType: provider?.auth_type ?? "api_key",
      extraHeaders: {},
    }

    const applyProfile = (profile: Auth.Profile) => {
      plan.profileId = profile.id
      if (profile.type === "api") {
        plan.authType = "api_key"
        if (profile.apiKey) plan.apiKey = profile.apiKey
      } else if (profile.type === "token") {
        plan.authType = "bearer"
        if (profile.token) plan.token = profile.token
      } else if (profile.type === "oauth") {
        plan.authType = "bearer"
        if (profile.token) plan.token = profile.token
      }
      if (profile.extraHeaders) plan.extraHeaders = { ...profile.extraHeaders }
    }

    // 1. Explicit authProfileId on the ref
    if (ref.authProfileId) {
      const profile = await Auth.getProfile(ref.authProfileId)
      if (profile) {
        plan.source = "profile"
        applyProfile(profile)
      }
    }

    // 2. Config provider block
    if (!plan.source && configProvider) {
      const opts = configProvider.options ?? {}
      if (opts.apiKey) {
        plan.source = "config"
        plan.apiKey = opts.apiKey
      } else if (configProvider.token) {
        plan.source = "config"
        plan.token = configProvider.token
        plan.authType = "bearer"
      } else if (configProvider.subprocess_cmd) {
        plan.source = "subprocess"
        plan.authType = "subprocess"
      }
    }

    // 3. Installed CLI brains need no gizzi API key — the CLI is already authed.
    if (!plan.source) {
      if (provider?.auth_type === "subprocess" || provider?.subprocess_cmd) {
        plan.source = "subprocess"
        plan.authType = "subprocess"
      } else if (provider?.auth_type === "none") {
        plan.source = "none"
        plan.authType = "none"
      }
    }

    // 4. Environment variables
    if (!plan.source && provider?.env?.length) {
      const env = Env.all()
      const apiKey = provider.env.map((item) => env[item]).find(Boolean)
      if (apiKey) {
        plan.source = "env"
        plan.apiKey = apiKey
      }
    }

    // 5. Auth-store profiles, in configured order
    if (!plan.source) {
      const profiles = await Auth.profilesForProvider(ref.providerID)
      const profile = profiles[0]
      if (profile) {
        plan.source = "profile"
        applyProfile(profile)
      }
    }

    // 6. Plugin OAuth loaders
    if (!plan.source && provider) {
      for (const plugin of await Plugin.list()) {
        if (!plugin.auth || plugin.auth.provider !== ref.providerID) continue
        if (!plugin.auth.loader) continue
        const auth = await Auth.get(ref.providerID)
        if (!auth) continue
        const loaded = await (plugin.auth.loader as unknown as RuntimeAuthLoader)(
          () => Auth.get(ref.providerID) as any,
          provider,
        )
        if (loaded?.apiKey || loaded?.token) {
          plan.source = "plugin"
          if (loaded.apiKey) plan.apiKey = loaded.apiKey
          if (loaded.token) {
            plan.token = loaded.token
            plan.authType = "bearer"
          }
          if (loaded.authType) plan.authType = loaded.authType as typeof plan.authType
          break
        }
      }
    }

    // Fallback when nothing resolved
    if (!plan.source) {
      plan.source = "none"
    }

    const baseURL = model ? loadBaseURL(model, options) : options["baseURL"]

    return {
      providerID: ref.providerID,
      modelID: ref.modelID,
      baseURL: baseURL ?? model?.api.url ?? "",
      authType: plan.authType ?? "api_key",
      extraHeaders: plan.extraHeaders ?? {},
      source: plan.source,
      ...(plan.apiKey !== undefined && { apiKey: plan.apiKey }),
      ...(plan.token !== undefined && { token: plan.token }),
      ...(plan.profileId !== undefined && { profileId: plan.profileId }),
    }
  }

  export async function resolveRuntimePolicy(ref: ModelRef): Promise<RuntimePolicy> {
    const [provider, model, cfg] = await Promise.all([
      getProvider(ref.providerID),
      getModel(ref.providerID, ref.modelID).catch(() => undefined),
      Config.get(),
    ])
    const configProvider = cfg.provider?.[ref.providerID]

    // 1. Per-model runtime config
    const modelRuntime = model?.options?.["runtime"] ?? configProvider?.models?.[ref.modelID]?.runtime
    if (modelRuntime === "sdk" || modelRuntime === "subprocess") return modelRuntime

    // 2. Per-provider runtime config
    const providerRuntime = configProvider?.runtime ?? provider?.options?.["runtime"]
    if (providerRuntime === "sdk" || providerRuntime === "subprocess") return providerRuntime

    // 3. auth_type/subprocess_cmd implies subprocess
    if (
      provider?.auth_type === "subprocess" ||
      provider?.subprocess_cmd ||
      configProvider?.auth_type === "subprocess" ||
      configProvider?.subprocess_cmd
    ) {
      return "subprocess"
    }

    // 4. Default to SDK
    return "sdk"
  }

  export type RotationState = {
    triedProfileIds: Set<string>
  }

  export function createRotationState(): RotationState {
    return { triedProfileIds: new Set() }
  }

  export async function rotateAuth(
    ref: ModelRef,
    state?: RotationState,
  ): Promise<AuthPlan | undefined> {
    const profiles = await Auth.profilesForProvider(ref.providerID)
    const next = profiles.find((p) => !state?.triedProfileIds.has(p.id))
    if (!next) return undefined
    state?.triedProfileIds.add(next.id)
    return prepareAuth({ ...ref, authProfileId: next.id })
  }

  export async function closest(providerID: string, query: string[]) {
    const s = await state()
    const provider = s.providers[providerID]
    if (!provider) return undefined
    for (const item of query) {
      for (const modelID of Object.keys(provider.models)) {
        if (modelID.includes(item))
          return {
            providerID,
            modelID,
          }
      }
    }
  }

  /** Subscription Fabric models run only on a human send (D16): never pick one automatically. */
  export function isFabricModel(model: { providerID: string }): boolean {
    return isFabricProviderID(model.providerID)
  }

  export async function getSmallModel(providerID: string) {
    const cfg = await Config.get()

    if (cfg.small_model) {
      const parsed = parseModel(cfg.small_model)
      return getModel(parsed.providerID, parsed.modelID)
    }

    const provider = await state().then((state) => state.providers[providerID])
    if (provider) {
      let priority = [
        "claude-haiku-4-5",
        "claude-haiku-4.5",
        "3-5-haiku",
        "3.5-haiku",
        "gemini-3-flash",
        "gemini-2.5-flash",
        "gpt-5-nano",
      ]
      if (providerID.startsWith("gizzi") || providerID.startsWith("gizziio")) {
        priority = ["gpt-5-nano"]
      }
      for (const item of priority) {
        for (const model of Object.keys(provider.models)) {
          if (model.includes(item)) return getModel(providerID, model)
        }
      }
    }

    // Check if gizzi/gizziio provider is available before using it
    const stateData = await state()
    const gizziProvider = stateData.providers["gizzi"] || stateData.providers["gizziio"]
    if (gizziProvider && gizziProvider.models["gpt-5-nano"]) {
      // Use the actual provider ID that exists (gizziio is the primary ID from models.dev)
      const providerID = stateData.providers["gizzi"] ? "gizzi" : "gizziio"
      return getModel(providerID, "gpt-5-nano")
    }

    // Fallback to embedded sidecar model if available
    const sidecarProvider = stateData.providers["sidecar"]
    if (sidecarProvider) {
      const sidecarModel = Object.keys(sidecarProvider.models)[0]
      if (sidecarModel) {
        log.info("using sidecar embedded model as small_model fallback", { model: sidecarModel })
        return getModel("sidecar", sidecarModel)
      }
    }

    return undefined
  }

  const priority = ["gpt-5", "claude-sonnet-4", "big-pickle", "gemini-3-pro"]
  export function sort(models: Model[]) {
    return sortBy(
      models,
      [(model) => priority.findIndex((filter) => model.id.includes(filter)), "desc"],
      [(model) => (model.id.includes("latest") ? 0 : 1), "asc"],
      [(model) => model.id, "desc"],
    )
  }

  export async function defaultModel() {
    const cfg = await Config.get()
    const providers = await list()

    // Explicit model config takes priority (lets users pin a specific model)
    if (cfg.model && cfg.model !== "auto" && cfg.model !== "auto/auto") {
      const parsed = parseModel(cfg.model)
      if (parsed.providerID !== "auto") {
        // A pinned model is an explicit user decision. Validate it and surface
        // ModelNotFoundError (with suggestions) instead of silently routing a
        // request to a different provider/model.
        await getModel(parsed.providerID, parsed.modelID)
        return parsed
      }
    }

    // Default: auto-route. Works as long as any provider is authenticated.
    // resolveAuto() picks the cheapest capable model per request at runtime.
    if (Object.keys(providers).length > 0) {
      return { providerID: "auto", modelID: "auto" }
    }

    // Nothing authenticated yet — tell the user
    throw new Error("No providers found. Run `gizzi auth add` to connect a provider.")
  }

  /** @deprecated kept for callers that need a concrete model ref for display purposes */
  export async function defaultModelConcrete() {
    const cfg = await Config.get()
    const providers = await list()
    if (cfg.model && cfg.model !== "auto" && cfg.model !== "auto/auto") {
      const parsed = parseModel(cfg.model)
      if (parsed.providerID !== "auto") {
        await getModel(parsed.providerID, parsed.modelID)
        return parsed
      }
    }

    const recent = (await Filesystem.readJson<{ recent?: { providerID: string; modelID: string }[] }>(
      path.join(GlobalPaths.state, "model.json"),
    )
      .then((x) => (Array.isArray(x.recent) ? x.recent : []))
      .catch(() => [])) as { providerID: string; modelID: string }[]
    for (const entry of recent) {
      const provider = providers[entry.providerID]
      if (!provider) continue
      if (!provider.models[entry.modelID]) continue
      return { providerID: entry.providerID, modelID: entry.modelID }
    }

    const provider = Object.values(providers).find((p) => !cfg.provider || Object.keys(cfg.provider).includes(p.id))
    if (!provider) throw new Error("no providers found")
    const [model] = sort(Object.values(provider.models))
    if (!model) throw new Error("no models found")
    return {
      providerID: provider.id,
      modelID: model.id,
    }
  }

  const MODEL_ALIASES: Record<string, string> = {
    "sonnet": "anthropic/claude-sonnet-4-6",
    "opus": "anthropic/claude-opus-4-6",
    "haiku": "anthropic/claude-haiku-4-5",
    "flash": "google/gemini-2.5-flash",
    "pro": "google/gemini-2.5-pro",
    "gpt4": "openai/gpt-4.1",
    "o3": "openai/o3",
    "o4-mini": "openai/o4-mini",
    "qwen": "local-mlx/qwen3.6-35b-a3b-4bit",
    "auto": "auto/auto",
  }

  export function parseModel(model: string): ModelRef {
    const aliased = MODEL_ALIASES[model.toLowerCase()]
    if (aliased) model = aliased

    const [providerID, ...rest] = model.split("/")
    return {
      providerID: providerID,
      modelID: rest.join("/"),
    }
  }

  // Per-session tier history for momentum (sessionID → last 5 tiers)
  const sessionMomentum = new Map<string, string[]>()

  // Cached auto-tier map — key is sorted provider IDs, invalidates when auth changes
  let autoTiersCache: {
    key: string
    tiers: Record<"simple" | "standard" | "complex" | "reasoning", { providerID: string; modelID: string }>
  } | null = null

  export async function buildAutoTiers(): Promise<
    Record<"simple" | "standard" | "complex" | "reasoning", { providerID: string; modelID: string }>
  > {
    const providers = await list()

    // Cache key: sorted provider IDs — changes when a new provider is authenticated
    const cacheKey = Object.keys(providers).sort().join(",")
    if (autoTiersCache?.key === cacheKey) return autoTiersCache.tiers

    // Flatten all active toolcall-capable models across available providers
    type Candidate = { providerID: string; modelID: string; costScore: number; isReasoning: boolean }
    const candidates: Candidate[] = []

    for (const [providerID, provider] of Object.entries(providers)) {
      for (const [modelID, model] of Object.entries(provider.models)) {
        if (model.status !== "active") continue
        if (!model.capabilities.toolcall) continue
        // cost.input/output are $ per 1M tokens; 0 = unknown (free/local)
        const costScore = model.cost.input + model.cost.output * 3
        candidates.push({
          providerID,
          modelID,
          costScore,
          isReasoning: model.capabilities.reasoning,
        })
      }
    }

    if (candidates.length === 0) {
      const fallback = await defaultModel()
      return { simple: fallback, standard: fallback, complex: fallback, reasoning: fallback }
    }

    // Separate known-reasoning models; sort the rest by cost ascending
    const reasoningCandidates = candidates.filter((c) => c.isReasoning)
    const general = candidates
      .filter((c) => !c.isReasoning && c.costScore > 0)
      .sort((a, b) => a.costScore - b.costScore)
    const free = candidates.filter((c) => !c.isReasoning && c.costScore === 0)

    // Build cost percentile buckets from priced models
    const count = general.length
    const p25 = Math.floor(count * 0.25)
    const p50 = Math.floor(count * 0.5)
    const p75 = Math.floor(count * 0.75)

    const pick = (arr: Candidate[]): Candidate | undefined => arr[0]

    const simplePick =
      pick(general.slice(0, Math.max(1, p25))) ??
      pick(free) ??
      pick(candidates)

    const standardPick =
      pick(general.slice(p25, Math.max(p25 + 1, p50))) ??
      simplePick!

    const complexPick =
      pick(sort(general.slice(p50, Math.max(p50 + 1, p75)).map((c) => ({
        id: c.modelID,
        providerID: c.providerID,
      } as any))).map((m: any) => general.find((c) => c.modelID === m.id)!).filter(Boolean)) ??
      pick(general.slice(p50, Math.max(p50 + 1, p75))) ??
      standardPick!

    // Reasoning: prefer flagged reasoning models (sorted by cost desc = most capable)
    const reasoningPick =
      pick(reasoningCandidates.sort((a, b) => b.costScore - a.costScore)) ??
      pick(general.slice(p75).reverse()) ??
      complexPick!

    const tiers = {
      simple:    { providerID: simplePick!.providerID,    modelID: simplePick!.modelID },
      standard:  { providerID: standardPick!.providerID,  modelID: standardPick!.modelID },
      complex:   { providerID: complexPick!.providerID,   modelID: complexPick!.modelID },
      reasoning: { providerID: reasoningPick!.providerID, modelID: reasoningPick!.modelID },
    }

    log.info("auto-tiers built", {
      simple:    `${tiers.simple.providerID}/${tiers.simple.modelID}`,
      standard:  `${tiers.standard.providerID}/${tiers.standard.modelID}`,
      complex:   `${tiers.complex.providerID}/${tiers.complex.modelID}`,
      reasoning: `${tiers.reasoning.providerID}/${tiers.reasoning.modelID}`,
    })

    autoTiersCache = { key: cacheKey, tiers }
    return tiers
  }

  export async function resolveAuto(
    messages: Array<{ role: string; content?: unknown }>,
    sessionID: string,
    tools?: Array<Record<string, unknown>>,
    toolChoice?: unknown,
  ): Promise<{ providerID: string; modelID: string }> {
    const { scoreRequest } = await import("@allternit/request-scorer")
    const cfg = await Config.get()

    // Build tier map: prefer explicit config, otherwise auto-detect from available models
    const tierMap = cfg.routing
      ? {
          simple:    parseModel(cfg.routing.tiers.simple),
          standard:  parseModel(cfg.routing.tiers.standard),
          complex:   parseModel(cfg.routing.tiers.complex),
          reasoning: parseModel(cfg.routing.tiers.reasoning),
        }
      : await buildAutoTiers()

    const recentTiers = (sessionMomentum.get(sessionID) ?? []) as any[]
    const result = scoreRequest(
      { messages: messages as any, tools: tools as any, tool_choice: toolChoice },
      undefined,
      recentTiers.length > 0 ? { recentTiers } : undefined,
    )

    const resolved = tierMap[result.tier as keyof typeof tierMap]

    // Record tier for next turn's momentum
    sessionMomentum.set(sessionID, [result.tier, ...recentTiers].slice(0, 5))

    log.info("auto-routed", {
      tier: result.tier,
      score: result.score,
      confidence: result.confidence,
      reason: result.reason,
      model: `${resolved.providerID}/${resolved.modelID}`,
      configSource: cfg.routing ? "manual" : "auto",
    })

    return resolved
  }

  export const ModelNotFoundError = NamedError.create(
    "ProviderModelNotFoundError",
    z.object({
      providerID: z.string(),
      modelID: z.string(),
      suggestions: z.array(z.string()).optional(),
    }),
  )

  export const InitError = NamedError.create(
    "ProviderInitError",
    z.object({
      providerID: z.string(),
    }),
  )
}

// Re-export so any module can register discovery hooks without reaching
// into the discovery internals.
export { Discovery } from "@/runtime/providers/discovery"
