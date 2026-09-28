import type { Provider } from "@/runtime/providers/provider"

/**
 * An installed CLI (claude, codex, kimi…) picks and validates its own model,
 * so a model id gizzi's built-in list doesn't have yet (a newer release, or
 * one the model picker offers) runs instead of failing with
 * ProviderModelNotFoundError. It takes the provider's first model as a
 * template and is remembered on the provider.
 */
export function cliModel(
  provider: Provider.Info,
  modelID: string,
  log?: { info: (message: string, extra?: Record<string, unknown>) => void },
): Provider.Model | undefined {
  if (provider.auth_type !== "subprocess" && !provider.subprocess_cmd) return undefined
  if (!modelID || modelID.includes("/")) return undefined
  const template = Object.values(provider.models)[0]
  if (!template) return undefined
  const model: Provider.Model = { ...template, id: modelID, name: modelID, api: { ...template.api, id: modelID }, variants: {} }
  provider.models[modelID] = model
  log?.info("cli model added", { providerID: provider.id, modelID })
  return model
}
