/**
 * Local Model Discovery
 *
 * Probes well-known localhost ports for running LLM HTTP servers.
 * Uses the OpenAI-compatible /v1/models endpoint where available.
 * Zero config — if Ollama or LM Studio is running, it appears in /model.
 *
 * Adding a new local server:
 *   Append to LOCAL_SERVERS below. Port, probe path, and model fallback
 *   are the only things needed.
 */

import type { DiscoveredProvider, DiscoveredModel } from "./index"

interface LocalServerSpec {
  id: string
  name: string
  port: number
  /** Path to GET for discovery. Should return OpenAI /v1/models JSON or any 200. */
  probe: string
  /** Whether the server has a real /v1/models endpoint */
  modelsEndpoint: boolean
  /** Fallback models if /v1/models is not available */
  fallbackModels: DiscoveredModel[]
}

const LOCAL_SERVERS: LocalServerSpec[] = [
  {
    id: "ollama",
    name: "Ollama (local)",
    port: 11434,
    probe: "/api/tags",
    modelsEndpoint: false, // uses /api/tags, not /v1/models
    fallbackModels: [{ id: "llama3.2", name: "Llama 3.2 (Ollama)", context: 128000, output: 8192 }],
  },
  {
    id: "lmstudio",
    name: "LM Studio (local)",
    port: 1234,
    probe: "/v1/models",
    modelsEndpoint: true,
    fallbackModels: [{ id: "local-model", name: "LM Studio model", context: 32768, output: 4096 }],
  },
  {
    id: "jan",
    name: "Jan (local)",
    port: 1337,
    probe: "/v1/models",
    modelsEndpoint: true,
    fallbackModels: [{ id: "local-model", name: "Jan model", context: 32768, output: 4096 }],
  },
  {
    id: "vllm",
    name: "vLLM (local)",
    port: 8000,
    probe: "/v1/models",
    modelsEndpoint: true,
    fallbackModels: [{ id: "local-model", name: "vLLM model", context: 32768, output: 8192 }],
  },
  {
    id: "llamacpp",
    name: "llama.cpp server (local)",
    port: 8080,
    probe: "/health",
    modelsEndpoint: false,
    fallbackModels: [{ id: "local-model", name: "llama.cpp model", context: 32768, output: 4096 }],
  },
  {
    id: "textgen-webui",
    name: "Text Generation WebUI (local)",
    port: 5000,
    probe: "/v1/models",
    modelsEndpoint: true,
    fallbackModels: [{ id: "local-model", name: "TextGen model", context: 32768, output: 4096 }],
  },
  {
    id: "gpt4all",
    name: "GPT4All (local)",
    port: 4891,
    probe: "/v1/models",
    modelsEndpoint: true,
    fallbackModels: [{ id: "local-model", name: "GPT4All model", context: 32768, output: 4096 }],
  },
]

async function probeServer(spec: LocalServerSpec): Promise<DiscoveredModel[] | null> {
  const base = `http://127.0.0.1:${spec.port}`
  try {
    const res = await fetch(`${base}${spec.probe}`, {
      signal: AbortSignal.timeout(1000),
    })
    if (!res.ok) return null

    // Ollama /api/tags — returns { models: [{ name, ... }] }
    if (spec.id === "ollama") {
      const data = await res.json() as { models?: { name: string; details?: { parameter_size?: string } }[] }
      const list = data.models ?? []
      // Running with nothing pulled: offer nothing, not a model that isn't there.
      if (list.length === 0) return []
      return list.map((m) => ({
        id: m.name,
        name: `${m.name}${m.details?.parameter_size ? ` (${m.details.parameter_size})` : ""}`,
        context: 128000,
        output: 8192,
      }))
    }

    // OpenAI-compat /v1/models — returns { data: [{ id, ... }] }
    if (spec.modelsEndpoint) {
      const data = await res.json() as { data?: { id: string }[] }
      const list = data.data ?? []
      // The server answered with an empty list: no model is loaded, so offer none.
      if (list.length === 0) return []
      return list.map((m) => ({ id: m.id, name: m.id, context: 32768, output: 4096 }))
    }

    // Health-only probe — just confirm it's up, use fallback model list
    return spec.fallbackModels
  } catch {
    return null
  }
}

export async function discoverLocalProviders(): Promise<DiscoveredProvider[]> {
  // LOCAL_SERVERS order, not probe-finish order.
  const found = await Promise.all(
    LOCAL_SERVERS.map(async (spec): Promise<DiscoveredProvider | undefined> => {
      const listed = await probeServer(spec)
      if (!listed || listed.length === 0) return
      // The server's own order; no default — the server doesn't name one.
      const models = listed.map((m, rank) => ({ ...m, rank }))
      return {
        id: spec.id,
        name: spec.name,
        auth_type: "none",
        base_url: `http://127.0.0.1:${spec.port}/v1`,
        source: "local",
        models,
      }
    }),
  )

  return found.filter((p): p is DiscoveredProvider => p !== undefined)
}
