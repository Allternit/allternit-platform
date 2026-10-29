import { Hono } from "hono"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import z from "zod/v4"
import { Config } from "@/runtime/context/config/config"
import { Provider } from "@/runtime/providers/provider"
import { ModelsDev } from "@/runtime/providers/adapters/models"
import { ProviderAuth } from "@/runtime/providers/adapters/auth"
import { mapValues } from "remeda"
import { errors } from "@/runtime/server/error"
import { lazy } from "@/shared/util/lazy"
import { Auth } from "@/runtime/integrations/auth"
import { MediaRegistry } from "@/runtime/providers/media"
import { ProviderQuotas } from "@/runtime/providers/quota"

export const ProviderRoutes = lazy(() =>
  new Hono()
    .get(
      "/",
      describeRoute({
        summary: "List providers",
        description: "Get a list of all available AI providers, including both available and connected ones.",
        operationId: "provider.list",
        responses: {
          200: {
            description: "List of providers",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        const config = await Config.get()
        const disabled = new Set(config.disabled_providers ?? [])
        const enabled = config.enabled_providers ? new Set(config.enabled_providers) : undefined

        const allProviders = await ModelsDev.get()
        const filteredProviders: Record<string, (typeof allProviders)[string]> = {}
        for (const [key, value] of Object.entries(allProviders)) {
          if (key === "vercel") continue
          if ((enabled ? enabled.has(key) : true) && !disabled.has(key)) {
            filteredProviders[key] = value
          }
        }

        // Phones serving a model come and go; a stale device list re-scans
        // here (capped at 1.5 s, skipped after a failed scan).
        await Provider.refreshAllternitDevices()
        const connected = await Provider.list()
        const providers = Object.assign(
          mapValues(filteredProviders, (x) => Provider.fromModelsDevProvider(x)),
          connected,
        )
        return c.json({
          all: Object.values(providers),
          default: mapValues(providers, (item) => Provider.sort(Object.values(item.models))[0].id),
          connected: Object.keys(connected),
        })
      },
    )
    .get(
      "/ollama/models",
      describeRoute({
        summary: "List local Ollama models",
        description: "Fetch models available in the local Ollama instance",
        operationId: "provider.ollama.models",
        responses: {
          200: {
            description: "List of Ollama models",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        try {
          const baseUrl = process.env.OLLAMA_BASE_URL || 'http://127.0.0.1:11434';
          const response = await fetch(`${baseUrl}/api/tags`);
          if (!response.ok) throw new Error('Ollama not reachable');
          const data = await response.json();
          return c.json(data);
        } catch (e) {
          return c.json({ models: [] });
        }
      }
    )
    .get(
      "/auth",
      describeRoute({
        summary: "Get provider auth methods",
        description: "Retrieve available authentication methods for all AI providers.",
        operationId: "provider.auth",
        responses: {
          200: {
            description: "Provider auth methods",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        return c.json(await ProviderAuth.methods())
      },
    )
    .post(
      "/video/generate",
      describeRoute({
        summary: "Generate a video with a runtime-owned provider credential",
        description: "Runs MiniMax video generation without exposing the provider API key to the browser or platform cloud.",
        operationId: "provider.video.generate",
        responses: { 200: { description: "Generated video", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      validator("json", z.object({
        prompt: z.string().min(1).max(20_000),
        model: z.string().max(120).optional(),
        duration: z.number().int().min(1).max(15).optional(),
        resolution: z.enum(["768p", "1080p"]).optional(),
        fps: z.number().int().min(1).max(60).optional(),
        aspectRatio: z.enum(["16:9", "9:16", "1:1", "4:3"]).optional(),
      })),
      async (c) => {
        const input = c.req.valid("json")
        const auth = await Auth.get("minimax")
        if (!auth || auth.type !== "api" || !auth.key) {
          return c.json({ error: "provider_not_connected", message: "Connect MiniMax in Models & Providers first." }, 409)
        }
        const providerHeaders = { Authorization: `Bearer ${auth.key}`, "Content-Type": "application/json" }
        const submit = await fetch("https://api.minimax.io/v1/video_generation", {
          method: "POST",
          headers: providerHeaders,
          body: JSON.stringify({
            model: input.model ?? "MiniMax-Hailuo-2.3",
            prompt: input.prompt,
            duration: input.duration ?? 6,
            resolution: (input.resolution ?? "1080p").toUpperCase(),
          }),
        })
        if (!submit.ok) {
          const detail = await submit.json().catch(() => ({}))
          return c.json({ error: "provider_error", message: detail?.message ?? submit.statusText }, 502)
        }
        const taskID = (await submit.json())?.task_id
        if (!taskID) return c.json({ error: "provider_error", message: "MiniMax returned no task ID." }, 502)

        const deadline = Date.now() + 5 * 60 * 1000
        while (Date.now() < deadline) {
          await new Promise((resolve) => setTimeout(resolve, 10_000))
          const status = await fetch(`https://api.minimax.io/v1/query/video_generation?task_id=${encodeURIComponent(taskID)}`, {
            headers: { Authorization: `Bearer ${auth.key}` },
          })
          if (!status.ok) continue
          const state = await status.json()
          if (state.status === "Fail") {
            return c.json({ error: "provider_error", message: state.error_message ?? "MiniMax generation failed." }, 502)
          }
          if (state.status !== "Success" || !state.file_id) continue
          const file = await fetch(`https://api.minimax.io/v1/files/retrieve?file_id=${encodeURIComponent(state.file_id)}`, {
            headers: { Authorization: `Bearer ${auth.key}` },
          })
          if (!file.ok) return c.json({ error: "provider_error", message: "MiniMax file retrieval failed." }, 502)
          const downloadURL = (await file.json())?.file?.download_url
          if (!downloadURL) return c.json({ error: "provider_error", message: "MiniMax returned no download URL." }, 502)
          return c.json({
            videos: [{
              id: `minimax_${taskID}`,
              url: downloadURL,
              prompt: input.prompt,
              metadata: {
                provider: "minimax",
                model: input.model ?? "MiniMax-Hailuo-2.3",
                duration: input.duration ?? 6,
                resolution: input.resolution ?? "1080p",
                fps: input.fps ?? 24,
                aspectRatio: input.aspectRatio ?? "16:9",
                createdAt: new Date().toISOString(),
              },
            }],
            prompt: input.prompt,
            config: { provider: "minimax", ...input },
          })
        }
        return c.json({ error: "provider_timeout", message: "MiniMax video generation timed out." }, 504)
      },
    )
    .get(
      "/media/providers",
      describeRoute({
        summary: "List media generation providers",
        description: "Get image/video/website generation providers and their auth requirements.",
        operationId: "provider.media.providers",
        responses: { 200: { description: "Media providers", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      async (c) => {
        return c.json(MediaRegistry.mediaProviders())
      },
    )
    .get(
      "/media/:mode/providers",
      describeRoute({
        summary: "List media providers for a mode",
        description: "Get providers that support a specific media mode (image, video, website, slides, sheets, doc).",
        operationId: "provider.media.mode.providers",
        responses: { 200: { description: "Media providers for mode", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      validator("param", z.object({ mode: z.string() })),
      async (c) => {
        const { mode } = c.req.valid("param")
        return c.json(MediaRegistry.providersForMode(MediaRegistry.ensureMode(mode)))
      },
    )
    .post(
      "/media/:mode/generate",
      describeRoute({
        summary: "Generate media with a runtime-owned provider credential",
        description: "Runs image/video/website generation without exposing the provider API key to the browser or platform cloud.",
        operationId: "provider.media.generate",
        responses: { 200: { description: "Generated media", content: { "application/json": { schema: resolver(z.any()) } } } },
      }),
      validator("param", z.object({ mode: z.string() })),
      validator("json", z.object({
        providerID: z.string(),
        prompt: z.string().min(1).max(20_000),
        model: z.string().max(120).optional(),
        width: z.number().int().optional(),
        height: z.number().int().optional(),
        aspectRatio: z.enum(["16:9", "9:16", "1:1", "4:3", "3:2", "2:3"]).optional(),
        duration: z.number().int().min(1).max(60).optional(),
        fps: z.number().int().min(1).max(120).optional(),
        quality: z.enum(["low", "medium", "high", "ultra"]).optional(),
        style: z.string().optional(),
        negativePrompt: z.string().optional(),
        seed: z.number().int().optional(),
        content: z.record(z.string(), z.any()).optional(),
        n: z.number().int().min(1).max(4).optional(),
      })),
      async (c) => {
        const { mode } = c.req.valid("param")
        const input = c.req.valid("json")
        try {
          const result = await MediaRegistry.generate(MediaRegistry.ensureMode(mode), input.providerID, input)
          return c.json(result)
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error)
          if (message.includes("Connect") && message.includes("first")) {
            return c.json({ error: "provider_not_connected", message }, 409)
          }
          if (message.includes("not reachable")) {
            return c.json({ error: "provider_unavailable", message }, 503)
          }
          return c.json({ error: "provider_error", message }, 502)
        }
      },
    )
    .get(
      "/:providerID/quota",
      describeRoute({
        summary: "Provider plan quota",
        description: "Rolling usage windows (e.g. 5-hour, weekly) for a subscription provider, from the provider's own sign-in.",
        operationId: "provider.quota",
        responses: {
          200: { description: "Quota result", content: { "application/json": { schema: resolver(z.any()) } } },
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { providerID } = c.req.valid("param") as any
        return c.json(await ProviderQuotas.get(providerID))
      },
    )
    .post(
      "/:providerID/oauth/authorize",
      describeRoute({
        summary: "OAuth authorize",
        description: "Initiate OAuth authorization for a specific AI provider to get an authorization URL.",
        operationId: "provider.oauth.authorize",
        responses: {
          200: {
            description: "Authorization URL and method",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(400),
        },
      }),
      validator("param", z.any()),
      async (c) => {
        const { providerID } = c.req.valid("param") as any
        const result = await ProviderAuth.authorize(providerID)
        return c.json(result)
      },
    )
    .post(
      "/:providerID/oauth/verify",
      describeRoute({
        summary: "OAuth verify",
        description: "Verify the OAuth authorization result from an AI provider.",
        operationId: "provider.oauth.verify",
        responses: {
          200: {
            description: "Verification result",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(400),
        },
      }),
      validator("param", z.any()),
      validator("json", z.any()),
      async (c) => {
        const { providerID } = c.req.valid("param") as any
        const input = c.req.valid("json") as any
        await ProviderAuth.callback({
          providerID,
          method: input.method ?? 0,
          code: input.code,
        })
        return c.json({ success: true })
      },
    ),
)
