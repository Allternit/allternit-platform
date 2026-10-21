/**
 * Model Providers
 */

export type ModelProvider = 'allternit' | 'openai' | 'google' | 'ollama'

export type APIProvider = 'firstParty'

export function getAPIProvider(): APIProvider {
  return 'firstParty'
}

export function getModelProvider(model: string): ModelProvider {
  if (model.startsWith('ollama/') || model.includes('llama') || model.includes('mistral') || model.includes('bonsai')) return 'ollama'
  if (model.includes('claude')) return 'allternit'
  if (model.includes('gpt')) return 'openai'
  return 'allternit'
}

export function isFirstPartyAllternitBaseUrl(): boolean {
  const url = process.env.ALLTERNIT_BASE_URL || ''
  if (!url) return true
  try {
    const host = new URL(url).host
    return host.includes('allternit.com') || host === 'localhost' || host === '127.0.0.1'
  } catch {
    return false
  }
}

/** Provider name string for telemetry/statsig tagging. */
export function getAPIProviderForStatsig(): APIProvider {
  return getAPIProvider()
}
