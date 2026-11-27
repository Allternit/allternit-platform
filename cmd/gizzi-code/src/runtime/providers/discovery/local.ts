/**
 * Local Model Discovery
 *
 * Probes well-known localhost ports for running LLM HTTP servers.
 * Uses the OpenAI-compatible /v1/models endpoint where available.
 * Zero config — if Ollama or LM Studio is running, it appears in /model.
 *
 * Adding a new local server:
 *   Append to LOCAL_SERVERS below. Port and probe path are the only things
 *   needed. A server shows only the models it lists itself: no placeholder
 *   rows for a server that is up but lists nothing.
 */

import type { DiscoveredProvider, DiscoveredModel } from "./index"

export interface LocalServerSpec {
  id: string
  name: string
  port: number
  /** Path to GET for discovery. Should return OpenAI /v1/models JSON or any 200. */
  probe: string
  /** Whether the server has a real /v1/models endpoint */
  modelsEndpoint: boolean
}

const LOCAL_SERVERS: LocalServerSpec[] = [
  {
    id: "ollama",
    name: "Ollama (local)",
    port: 11434,
    probe: "/api/tags",
    modelsEndpoint: false, // uses /api/tags, not /v1/models
  },
  {
    id: "lmstudio",
    name: "LM Studio (local)",
    port: 1234,
    probe: "/v1/models",
    modelsEndpoint: true,
  },
  {
    id: "jan",
    name: "Jan (local)",
    port: 1337,
    probe: "/v1/models",
    modelsEndpoint: true,
  },
  {
    id: "vllm",
    name: "vLLM (local)",
    port: 8000,
    probe: "/v1/models",
    modelsEndpoint: true,
  },
  {
    id: "llamacpp",
    name: "llama.cpp server (local)",
    port: 8080,
    probe: "/v1/models", // llama-server serves the OpenAI list; /health said "up" and nothing else
    modelsEndpoint: true,
  },
  {
    id: "textgen-webui",
    name: "Text Generation WebUI (local)",
    port: 5000,
    probe: "/v1/models",
    modelsEndpoint: true,
  },
  {
    id: "gpt4all",
    name: "GPT4All (local)",
    port: 4891,
    probe: "/v1/models",
    modelsEndpoint: true,
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

    // Up but no way to list models: offer none.
    return []
  } catch {
    return null
  }
}

export async function discoverLocalProviders(servers: LocalServerSpec[] = LOCAL_SERVERS): Promise<DiscoveredProvider[]> {
  // LOCAL_SERVERS order, not probe-finish order.
  const found = await Promise.all(
    servers.map(async (spec): Promise<DiscoveredProvider | undefined> => {
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

/** localhost / 127.0.0.1 / ::1: a server the user runs on this machine. */
export function isLocalBaseURL(url: string | undefined): boolean {
  if (!url) return false
  try {
    const host = new URL(url).hostname
    return host === "localhost" || host === "127.0.0.1" || host === "::1" || host === "[::1]"
  } catch {
    return false
  }
}

/**
 * Does the OpenAI-compatible server at `baseURL` (a config `options.baseURL`,
 * usually ending in /v1) answer within 1 s? Any HTTP answer counts as up;
 * only a refused connection or a timeout is offline.
 */
export async function localServerAnswers(baseURL: string, timeoutMs = 1000): Promise<boolean> {
  try {
    await fetch(`${baseURL.replace(/\/+$/, "")}/models`, { signal: AbortSignal.timeout(timeoutMs) })
    return true
  } catch {
    return false
  }
}
