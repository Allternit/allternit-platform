import { Auth } from "@/runtime/integrations/auth"
import { Log } from "@/shared/util/log"
import type {
  MediaArtifact,
  MediaDriver,
  MediaGenerateInput,
  MediaGenerateResult,
  MediaMode,
  MediaProvider,
} from "./types"
import { MediaMode as MediaModeEnum } from "./types"
import * as pollinations from "./drivers/pollinations"
import * as bonsai from "./drivers/bonsai"
import * as openaiImage from "./drivers/openai-image"
import * as minimax from "./drivers/minimax"

const log = Log.create({ service: "media-providers" })

// Static registry. Each entry declares what it can create and how it
// authenticates. Availability is resolved lazily so free providers always
// appear and local/subprocess providers only appear when the binary is found.
export const MEDIA_PROVIDERS: MediaProvider[] = [
  {
    id: "pollinations-image",
    name: "Pollinations AI",
    modes: ["image"],
    authType: "none",
    description: "Free, no-signup image generation via pollinations.ai",
    tier: "free",
  },
  {
    id: "pollinations-video",
    name: "Pollinations Video",
    modes: ["video"],
    authType: "none",
    description: "Free, no-signup video generation via pollinations.ai",
    tier: "free",
  },
  {
    id: "bonsai-local",
    name: "Bonsai Local",
    modes: ["image"],
    authType: "none",
    description: "Local image generation when a Bonsai-compatible local engine is running",
    tier: "free",
  },
  {
    id: "openai-image",
    name: "OpenAI Image",
    modes: ["image"],
    authType: "api_key",
    authProviderID: "openai",
    description: "DALL-E 3 and GPT-image models",
    tier: "standard",
  },
  {
    id: "minimax",
    name: "MiniMax",
    modes: ["video"],
    authType: "api_key",
    authProviderID: "minimax",
    description: "MiniMax Hailuo video generation",
    tier: "standard",
  },
]

function driverFor(providerID: string): MediaDriver | undefined {
  switch (providerID) {
    case "pollinations-image":
      return pollinations.imageDriver()
    case "pollinations-video":
      return pollinations.videoDriver()
    case "bonsai-local":
      return bonsai.localDriver()
    case "openai-image":
      return openaiImage.driver()
    case "minimax":
      return minimax.driver()
  }
}

export function mediaProviders(): MediaProvider[] {
  return MEDIA_PROVIDERS.map((p) => ({ ...p, available: isAvailable(p) }))
}

export function providersForMode(mode: MediaMode): MediaProvider[] {
  return mediaProviders().filter((p) => p.modes.includes(mode))
}

function isAvailable(provider: MediaProvider): boolean {
  if (provider.authType === "none") return true
  return true // availability resolved at generate time for API-key providers
}

export async function generate(
  mode: MediaMode,
  providerID: string,
  input: MediaGenerateInput,
): Promise<MediaGenerateResult> {
  const provider = MEDIA_PROVIDERS.find((p) => p.id === providerID)
  if (!provider) {
    throw new Error(`Unknown media provider: ${providerID}`)
  }
  if (!provider.modes.includes(mode)) {
    throw new Error(`Provider ${providerID} does not support ${mode}`)
  }

  if (provider.authType === "api_key") {
    const auth = await Auth.get(provider.authProviderID ?? providerID)
    if (!auth || auth.type !== "api" || !auth.key) {
      throw new Error(`Connect ${provider.name} in Models & Providers first.`)
    }
  }

  const driver = driverFor(providerID)
  if (!driver) {
    throw new Error(`Driver for ${providerID} is not implemented yet.`)
  }

  using _ = log.time("media.generate", { mode, providerID })
  const result = await driver.generate(input)
  return {
    ...result,
    config: { provider: providerID, mode, ...input, ...(result.config ?? {}) },
  }
}

export function artifactFromUrl(
  id: string,
  url: string,
  mimeType: string,
  metadata?: Record<string, unknown>,
): MediaArtifact {
  return {
    id,
    url,
    mimeType,
    metadata,
  }
}

export function ensureMode(value: string): MediaMode {
  const parsed = MediaModeEnum.safeParse(value)
  if (!parsed.success) {
    throw new Error(`Invalid media mode: ${value}. Must be one of ${MediaModeEnum.options.join(", ")}`)
  }
  return parsed.data
}
