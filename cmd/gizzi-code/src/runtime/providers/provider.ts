import { nativeToolsetFetch } from "@/runtime/tools/computer-toolset/anthropic-native"
import z from "zod/v4"
import fuzzysort from "fuzzysort"
import { Config } from "@/runtime/context/config/config"
import { mapValues, mergeDeep, omit, pickBy } from "remeda"
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
import { SelfhostInference } from "@/runtime/providers/selfhost-inference"
import { CUSTOM_LOADERS } from "@/runtime/providers/adapters/loaders"
import type { CustomModelLoader } from "@/runtime/providers/types"
import { Discovery, type DiscoveredProvider } from "@/runtime/providers/discovery"
import { Installation } from "@/shared/installation"
import { cloudInferenceToken, getCachedAllternitPlan } from "@/runtime/providers/discovery/allternit-cloud"
import { ModelList } from "@/runtime/providers/model-list"
import { resolveDefaultBrain, type DefaultBrain } from "@/runtime/providers/default-brain"
import { SubprocessLanguageModel } from "@/runtime/providers/adapters/loaders/subprocess"
import { SubscriptionFabricLanguageModel } from "@/runtime/providers/fabric/language-model"
import { fabricConfigured, isFabricProviderID } from "@/runtime/providers/fabric/client"
import { PersonPlanLanguageModel } from "@/runtime/providers/person-plan/language-model"
import { PERSON_PLAN_RUNTIME } from "@/runtime/providers/person-plan/discovery"
import { isPersonOnlyProvider } from "@/runtime/providers/person-plan/rules"
import { discoverSubscriptionFabric } from "@/runtime/providers/fabric/discovery"
import { cliModel, stripProviderPrefix } from "@/runtime/providers/cli-model"
import { tapRetryHint } from "@/runtime/providers/retry-hint"
import { ProviderQuotas } from "@/runtime/providers/quota"

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
    source: "profile" | "config" | "env" | "auth" | "plugin" | "subprocess" | "none"
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

  function selfhostModel(cfg: SelfhostInference.Config, spec: SelfhostInference.Spec, name: string): Model {
    const apiID = SelfhostInference.resolveModel(cfg, name)
    return {
      id: name,
      providerID: spec.id,
      name,
      api: { id: apiID, url: spec.url, npm: spec.npm(apiID) },
      status: "active",
      headers: {},
      options: {},
      cost: { input: 0, output: 0, cache: { read: 0, write: 0 } },
      limit: { context: 200000, output: 32000 },
      capabilities: {
        temperature: true,
        reasoning: true,
        attachment: true,
        toolcall: true,
        input: { text: true, audio: false, image: true, video: false, pdf: true },
        output: { text: true, audio: false, image: false, video: false, pdf: false },
        interleaved: false,
      },
      release_date: "",
      variants: {},
    }
  }

  /** Self-hosted inference: one provider, the model_map keys as its models, no catalog, no discovery. */
  function selfhostState(cfg: SelfhostInference.Config) {
    const spec = SelfhostInference.spec(cfg)
    const info: Info = {
      id: spec.id,
      name: spec.name,
      source: "config",
      env: [],
      options: spec.options,
      models: {},
    }
    for (const name of SelfhostInference.listed(cfg)) info.models[name] = selfhostModel(cfg, spec, name)
    return {
      models: new Map<string, LanguageModelV2>(),
      providers: { [spec.id]: info } as { [providerID: string]: Info },
      sdk: new Map<number, SDK>(),
      modelLoaders: {} as { [providerID: string]: CustomModelLoader },
    }
  }

  const state = Instance.state(async () => {
    const selfhost = SelfhostInference.load()
    if (selfhost) {
      log.info("self-hosted inference", { provider: selfhost.provider, models: SelfhostInference.listed(selfhost).length })
      return selfhostState(selfhost)
    }
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
      const dpModels = discoveredModels(dp)
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
        key: dp.id === "allternit" ? await cloudInferenceToken() : undefined,
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

    // Subscription Fabric providers appear once the user binds a Sessions
    // computer — usually after gizzi started. Until one is found, re-check the
    // catalog in the background and add it to the live provider list.
    if (!Object.keys(providers).some(isFabricProviderID)) watchForFabricProviders(providers, isProviderAllowed)

    return {
      models: languages,
      providers,
      sdk,
      modelLoaders,
    }
  })

  const FABRIC_RECHECK_MS = 60_000

  function watchForFabricProviders(providers: Record<string, Info>, isProviderAllowed: (id: string) => boolean) {
    if (!fabricConfigured()) return
    const timer = setInterval(async () => {
      const found = await discoverSubscriptionFabric().catch(() => [])
      if (found.length === 0) return
      clearInterval(timer)
      for (const dp of found) {
        if (!isProviderAllowed(dp.id) || providers[dp.id]) continue
        const models = discoveredModels(dp)
        for (const model of Object.values(models)) {
          model.variants = mapValues(ProviderTransform.variants(model), (v) => v)
        }
        providers[dp.id] = {
          id: dp.id,
          name: dp.name,
          source: "custom",
          env: [],
          auth_type: dp.auth_type,
          options: { ...(dp.options ?? {}) },
          models,
        }
        log.info("discovered-late", { providerID: dp.id, models: Object.keys(models).length })
      }
    }, FABRIC_RECHECK_MS)
    timer.unref?.()
  }

  /** Subscription lanes (web-chat fabric) don't run gizzi's tools or take temperature. */
  function isSubscriptionRuntime(runtime: unknown): boolean {
    return runtime === "fabric"
  }

  /** Lanes that run no gizzi tools: Subscription Fabric and the person's own plans (chat only). */
  function isToollessRuntime(runtime: unknown): boolean {
    return runtime === "fabric" || runtime === PERSON_PLAN_RUNTIME
  }

  function discoveredModels(dp: DiscoveredProvider): Record<string, Model> {
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
          temperature: !isToollessRuntime(dp.options?.["runtime"]),
          reasoning: false,
          attachment: false,
          // Fabric chat runs in the provider's web UI: gizzi's tools cannot be
          // called there. Also keeps these models out of auto-tier picks.
          toolcall: !isToollessRuntime(dp.options?.["runtime"]),
          input:  { text: true, audio: false, image: false, video: false, pdf: false },
          // Fabric replies can carry files the provider made (images,
          // decks, documents); they arrive as file parts on the message.
          output: {
            text: true,
            audio: false,
            image: isSubscriptionRuntime(dp.options?.["runtime"]),
            video: false,
            pdf: isSubscriptionRuntime(dp.options?.["runtime"]),
          },
          interleaved: false,
        },
        cost: { input: 0, output: 0, cache: { read: 0, write: 0 } },
        limit: { context: m.context ?? 32768, output: m.output ?? 4096 },
        variants: {},
      }
    }
    return dpModels
  }

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
        // A built-in SDK model (openai, anthropic) has no catalog URL, so the plan's is "";
        // an empty baseURL makes every request fail ("fetch() URL is invalid"). Use the default.
        const baseURL = plan.baseURL || loadBaseURL(model, options)
        if (baseURL) options["baseURL"] = baseURL
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
      // Lets the Cloud tell Gizzi Code traffic apart in usage reports (client=gizzi-code).
      if (model.providerID === "allternit")
        options["headers"] = {
          ...options["headers"],
          "X-Allternit-Client": `gizzi-code/${Installation.VERSION}`,
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
        const wrapRetryHint = (res: Response) => {
          // Usage windows a plan reports per response (wrap-up reads them between steps).
          ProviderQuotas.recordHeaders(model.providerID, res.headers)
          return tapRetryHint({
            response: res,
            method: opts.method,
            url: retryHintUrl,
            sessionID: retryHintSession,
            npm: model.api.npm,
          })
        }

        // Computer toolset, Claude-native: swap our `computer`/`browser`
        // function tools for computer_toolset_20260801 /
        // browser_toolset_20260801 on the wire (falls back to the function
        // tools when the model rejects them). See computer-toolset/anthropic-native.ts.
        const sendRequest = () =>
          model.api.npm === "@ai-sdk/anthropic" &&
          opts.method === "POST" &&
          typeof opts.body === "string" &&
          opts.body.includes("[allternit.")
            ? nativeToolsetFetch(opts.body, (body) => fetchFn(input, { ...reqInit, body }))
            : fetchFn(input, reqInit)

        // Optional per-provider concurrency cap (provider.options.concurrency).
        const cap = Number(options["concurrency"])
        if (Number.isFinite(cap) && cap > 0) {
          const sem = providerSemaphore(model.providerID, Math.floor(cap))
          await sem.acquire()
          try {
            return wrapRetryHint(await sendRequest())
          } finally {
            sem.release()
          }
        }

        return wrapRetryHint(await sendRequest())
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
    const selfhost = SelfhostInference.load()
    if (selfhost) {
      // A name outside model_map (or a provider this server did not register) fails here, before any network call.
      const spec = SelfhostInference.spec(selfhost)
      if (providerID !== spec.id) throw new SelfhostInference.ModelNotMappedError(modelID, SelfhostInference.listed(selfhost))
      const s = await state()
      const apiID = SelfhostInference.resolveModel(selfhost, modelID)
      const provider = s.providers[spec.id]
      // Own-id passthrough (non-gateway, empty model_map): remember the model on the provider.
      return (provider.models[modelID] ??= { ...selfhostModel(selfhost, spec, modelID), api: { id: apiID, url: spec.url, npm: spec.npm(apiID) } })
    }
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
    const selfhost = SelfhostInference.load()
    if (selfhost) SelfhostInference.resolveModel(selfhost, model.id)
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

    // The person's own plans (chatgpt/…, plan/…, copilot-plan/…): chat only,
    // and only on a turn the person sent (person-plan/language-model.ts).
    if (provider?.options?.["runtime"] === PERSON_PLAN_RUNTIME || isPersonOnlyProvider(model.providerID)) {
      const language = new PersonPlanLanguageModel(model.providerID, model.api.id) as unknown as LanguageModelV2
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
    const selfhost = SelfhostInference.load()
    if (selfhost) {
      // Credentials come from the server's inference config only; stored auth, env keys and profiles never apply.
      const model = await getModel(ref.providerID, ref.modelID)
      return {
        providerID: ref.providerID,
        modelID: ref.modelID,
        baseURL: model.api.url,
        authType: "api_key",
        extraHeaders: {},
        source: "config",
      }
    }
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

    // 5b. A plain API key in the auth store (`PUT /auth/{provider}`, `gizzi auth login`):
    // the provider list counts it as connected, so the request must carry it.
    if (!plan.source) {
      const stored = await Auth.get(ref.providerID)
      if (stored?.type === "api" && stored.key) {
        plan.source = "auth"
        plan.authType = "api_key"
        plan.apiKey = stored.key
      }
    }

    // 5c. Allternit Cloud: ALLTERNIT_API_TOKEN (the device token Desktop and every
    // cloud computer hand gizzi) or this runtime's own paired device token. The
    // provider was loaded with this key, but a turn always carries a plan and the
    // SDK takes only the plan's key, so without this every Cloud turn on a computer
    // with no ALLTERNIT_API_KEY (a Platform runtime, a cloud computer) went out
    // unsigned and failed "Unauthorized". Read fresh: device tokens rotate.
    if (!plan.source && ref.providerID === "allternit") {
      const token = await cloudInferenceToken()
      if (token) {
        plan.source = "env"
        plan.authType = "api_key"
        plan.apiKey = token
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

  /**
   * Models that run only on the person's own send: Subscription Fabric (D16)
   * and the person's plans (chatgpt/…, plan/…, copilot-plan/…). Never picked
   * for a title, a summary, a small model or any other helper call.
   */
  export function isHumanSendOnly(model: { providerID: string }): boolean {
    return isFabricProviderID(model.providerID) || isPersonOnlyProvider(model.providerID)
  }

  export async function getSmallModel(providerID: string) {
    const cfg = await Config.get()

    if (cfg.small_model) {
      const parsed = parseModel(cfg.small_model)
      // A small_model whose provider/model is gone (CLI uninstalled, local
      // server off) falls through to the automatic pick instead of failing the turn.
      const configured = await state().then((s) => s.providers[parsed.providerID]?.models[parsed.modelID])
      if (configured) return getModel(parsed.providerID, parsed.modelID)
      log.warn("small_model not available, picking one", { small_model: cfg.small_model })
    }

    const provider = await state().then((state) => state.providers[providerID])
    if (provider) {
      let priority = [
        "claude-haiku-5-5",
        "claude-haiku",
        "gemini-3.8-flash",
        "gemini-3",
        "flash",
        "gpt-6-luna",
        "luna",
        "mini",
        "nano",
      ]
      if (providerID.startsWith("gizzi") || providerID.startsWith("gizziio")) {
        priority = ["gpt-5-nano"]
      }
      // Match at a word start so "mini" never matches "gemini".
      const matches = (model: string, item: string) =>
        new RegExp(`(^|[^a-z0-9])${item.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`, "i").test(model)
      for (const item of priority) {
        for (const model of Object.keys(provider.models)) {
          if (matches(model, item)) return getModel(providerID, model)
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

  /**
   * Model order: for a catalog CLI the vendor's flagged default first, then
   * the vendor's order; then current families (gpt-6, claude-sonnet-5,
   * gemini-3, kimi), `latest`, id. One implementation in ModelList.
   */
  export function sort<T>(models: T[]): T[] {
    // Callers pass registry models and loosely typed route payloads; every entry has an `id`.
    return ModelList.sortModels(models as unknown as { id: string; providerID?: string }[]) as unknown as T[]
  }

  const AUTO_REF = { providerID: "auto", modelID: "auto" }

  /** The shared default-brain resolver (default-brain.ts) over this config and discovery. */
  function resolveBrain(cfg: Config.Info): DefaultBrain {
    const disabled = new Set(cfg.disabled_providers ?? [])
    const enabled = cfg.enabled_providers ? new Set(cfg.enabled_providers) : undefined
    return resolveDefaultBrain({
      modelAuto: cfg.model_auto,
      currentModel: cfg.model,
      plan: getCachedAllternitPlan(),
      providers: Discovery.last().filter((p) => !disabled.has(p.id) && (!enabled || enabled.has(p.id))),
    })
  }

  /** The resolver's model as a ref when it is a pin (validated) or a model this machine lists. */
  async function resolvedRef(cfg: Config.Info, providers: Record<string, Info>): Promise<ModelRef | undefined> {
    const brain = resolveBrain(cfg)
    if (!brain.model) return undefined
    const parsed = parseModel(brain.model)
    if (parsed.providerID === "auto") return undefined
    if (brain.pinned) {
      // A pinned model is an explicit user decision. Validate it and surface
      // ModelNotFoundError (with suggestions) instead of silently routing a
      // request to a different provider/model.
      await getModel(parsed.providerID, parsed.modelID)
      return parsed
    }
    return providers[parsed.providerID]?.models[parsed.modelID] ? parsed : undefined
  }

  function isAutoRef(model: string | undefined) {
    return model === "auto" || model === "auto/auto"
  }

  export async function defaultModel() {
    const cfg = await Config.get()
    // list() runs discovery first, so the resolver sees this machine's CLIs.
    const providers = await list()

    // Explicit "auto": per-request routing (resolveAuto).
    if (!isAutoRef(cfg.model)) {
      const ref = await resolvedRef(cfg, providers)
      if (ref) return ref
    }

    // Fallback: auto-route. Works as long as any provider is authenticated.
    // resolveAuto() picks the cheapest capable model per request at runtime.
    if (Object.keys(providers).length > 0) {
      return AUTO_REF
    }

    // Nothing authenticated yet — tell the user
    throw new Error("No providers found. Run `gizzi auth add` to connect a provider.")
  }

  type RecentModel = { providerID: string; modelID: string }
  const RECENT_MAX = 20

  function modelStateFile() {
    return path.join(GlobalPaths.state, "model.json")
  }

  async function readRecent(): Promise<RecentModel[]> {
    return (await Filesystem.readJson<{ recent?: RecentModel[] }>(modelStateFile())
      .then((x) => (Array.isArray(x.recent) ? x.recent : []))
      .catch(() => [])) as RecentModel[]
  }

  /** Record a model the user ran: `state/model.json` `recent`, newest first, unique, at most 20. */
  export async function recordRecentModel(ref: { providerID: string; modelID: string }): Promise<void> {
    if (!ref.providerID || !ref.modelID || ref.providerID === "auto") return
    const file = modelStateFile()
    const existing = await Filesystem.readJson<Record<string, unknown>>(file).catch(() => ({}) as Record<string, unknown>)
    const base = existing && typeof existing === "object" && !Array.isArray(existing) ? existing : {}
    const previous = Array.isArray(base["recent"]) ? (base["recent"] as RecentModel[]) : []
    const recent = [
      { providerID: ref.providerID, modelID: ref.modelID },
      ...previous.filter(
        (entry) =>
          entry &&
          typeof entry.providerID === "string" &&
          typeof entry.modelID === "string" &&
          !(entry.providerID === ref.providerID && entry.modelID === ref.modelID),
      ),
    ].slice(0, RECENT_MAX)
    await Filesystem.writeJson(file, { ...base, recent }).catch((err) =>
      log.warn("failed to record recent model", { error: err }),
    )
  }

  /** @deprecated kept for callers that need a concrete model ref for display purposes */
  export async function defaultModelConcrete() {
    const cfg = await Config.get()
    const providers = await list()
    if (!isAutoRef(cfg.model)) {
      const ref = await resolvedRef(cfg, providers)
      if (ref) return ref
    }

    const recent = await readRecent()
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
    "sonnet": "anthropic/claude-sonnet-5-5",
    "opus": "anthropic/claude-opus-5-5",
    "haiku": "anthropic/claude-haiku-5-5",
    "fable": "anthropic/claude-fable-5-2",
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
