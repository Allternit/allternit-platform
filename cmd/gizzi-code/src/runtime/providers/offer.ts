import { findSubprocessSpec } from "@/runtime/providers/discovery/subprocess"

/**
 * Which providers a model picker offers, and in what order. One rule for
 * every gizzi surface (the TUI picker, `gizzi models`), the same one
 * Desktop's picker uses (allternit-ai ModelPickerPopover `isOfferedProvider`):
 *
 *   1. Allternit Cloud — the combined routing catalog.
 *   2. Your subscriptions — CLI sign-ins (claude-cli, codex-cli, kimi-cli…)
 *      and Subscription Fabric (`subs-*`).
 *   3. API keys you connected.
 *   4. Local runtimes on this computer.
 *
 * Aggregators such as OpenRouter are how Allternit Cloud routes, never a
 * standalone entry. A model ref that names one still runs when asked for
 * by name (`--model openrouter/…`); it is only never offered.
 */
export namespace ProviderOffer {
  export type Kind = "cloud" | "subscription" | "api" | "local"

  /** Picker section order. */
  export const ORDER: readonly Kind[] = ["cloud", "subscription", "api", "local"]

  export const TITLE: Record<Kind, string> = {
    cloud: "Allternit Cloud",
    subscription: "Your subscriptions",
    api: "API keys",
    local: "Local",
  }

  const NEVER_STANDALONE = new Set(["openrouter"])
  /** Runtimes on this computer even when a CLI drives them (Desktop's LOCAL_PROVIDERS). */
  const LOCAL_PROVIDERS = new Set(["ollama", "ollama-cli", "omlx", "allternit-sidecar", "local-brain"])

  export interface Candidate {
    id: string
    /** Discovery source ("platform" | "subprocess" | "subscription" | "local" | "plugin") or Provider.Info source. */
    source?: string
    auth_type?: string
    baseURL?: string
    options?: Record<string, unknown>
  }

  export function neverStandalone(providerID: string): boolean {
    return NEVER_STANDALONE.has(providerID) || providerID.startsWith("openrouter")
  }

  /** Subscription Fabric providers: the user's web subscription on their Sessions computer. */
  export function isFabric(providerID: string, options?: Record<string, unknown>): boolean {
    return options?.["runtime"] === "fabric" || providerID.startsWith("subs-")
  }

  /**
   * A provider whose turns run in the user's own installed CLI (the shared
   * CLI catalog plus CLIs the user added), not over an HTTP API.
   */
  export function isCliBrain(providerID: string): boolean {
    const spec = findSubprocessSpec(providerID)
    return Boolean(spec && spec.adapter !== "unsupported")
  }

  function isLoopback(url: string | undefined): boolean {
    if (!url) return false
    try {
      const host = new URL(url).hostname
      return host === "localhost" || host === "127.0.0.1" || host === "::1" || host === "[::1]" || host.endsWith(".local")
    } catch {
      return false
    }
  }

  export function kind(p: Candidate): Kind {
    if (p.id === "allternit" || p.source === "platform") return "cloud"
    if (LOCAL_PROVIDERS.has(p.id)) return "local"
    if (
      p.source === "subprocess" ||
      p.source === "subscription" ||
      p.auth_type === "subprocess" ||
      isCliBrain(p.id) ||
      isFabric(p.id, p.options)
    )
      return "subscription"
    if (p.source === "local" || p.auth_type === "none" || isLoopback(p.baseURL ?? (p.options?.["baseURL"] as string | undefined)))
      return "local"
    return "api"
  }

  /**
   * Whether a picker lists this provider. Cloud, subscriptions and local
   * runtimes are always offered; an API provider only once the user
   * connected it (`connected`: a key or sign-in is on file).
   */
  export function isOffered(p: Candidate, auth?: { connected: boolean }): boolean {
    if (neverStandalone(p.id)) return false
    if (kind(p) !== "api") return true
    return Boolean(auth?.connected)
  }

  export function rank(p: Candidate): number {
    return ORDER.indexOf(kind(p))
  }
}
