// Bonsai local/WebGPU image generation driver.
// Probes a local Bonsai-compatible HTTP endpoint and falls back gracefully.

import type { MediaDriver, MediaGenerateInput, MediaGenerateResult, MediaArtifact } from "../types"

const DEFAULT_BASE_URL = "http://127.0.0.1:11435"

function aspectToSize(aspectRatio?: string): { width: number; height: number } {
  switch (aspectRatio) {
    case "9:16":
      return { width: 576, height: 1024 }
    case "1:1":
      return { width: 1024, height: 1024 }
    case "4:3":
      return { width: 1024, height: 768 }
    case "2:3":
      return { width: 768, height: 1152 }
    case "3:2":
      return { width: 1152, height: 768 }
    case "16:9":
    default:
      return { width: 1024, height: 576 }
  }
}

export function localDriver(): MediaDriver {
  return {
    async generate(input: MediaGenerateInput): Promise<MediaGenerateResult> {
      const baseUrl = process.env.BONSAI_BASE_URL || DEFAULT_BASE_URL
      const size = aspectToSize(input.aspectRatio)
      const width = input.width ?? size.width
      const height = input.height ?? size.height

      const probe = await fetch(`${baseUrl}/health`, { signal: AbortSignal.timeout(3000) }).catch(() => undefined)
      if (!probe || !probe.ok) {
        throw new Error(
          `Bonsai local engine is not reachable at ${baseUrl}. Start a Bonsai-compatible server or set BONSAI_BASE_URL.`,
        )
      }

      const response = await fetch(`${baseUrl}/generate`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          prompt: input.prompt,
          width,
          height,
          seed: input.seed ?? Math.floor(Math.random() * 1_000_000),
          negative_prompt: input.negativePrompt,
          style: input.style,
        }),
        signal: AbortSignal.timeout(120_000),
      })

      if (!response.ok) {
        const detail = await response.json().catch(() => ({}))
        throw new Error(detail.message || `Bonsai generation failed: ${response.statusText}`)
      }

      const body = (await response.json()) as { url?: string; data?: string }
      const artifact: MediaArtifact = {
        id: `bonsai_${Date.now()}`,
        url: body.url,
        data: body.data,
        mimeType: "image/png",
        metadata: { provider: "bonsai-local", width, height, prompt: input.prompt },
      }
      return { artifacts: [artifact], prompt: input.prompt }
    },
  }
}
