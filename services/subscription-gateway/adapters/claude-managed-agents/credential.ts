// Credential plumbing for a user-owned Anthropic API key. Allternit never has its own key here.
// The gateway host (src/aai/call-scope.ts) parks the per-call { binding, credential } in AsyncLocalStorage under a
// global symbol; this module reads it without importing gateway internals. Keys are never cached across bindings.
import { createHash } from "node:crypto";
import { AsyncLocalStorage } from "node:async_hooks";

export interface BindingRef { id: string }
export interface CredentialCtx { op: string; agentId?: string; contextId?: string }
export interface ApiCredential { apiKey: string }
export type ResolveCredential = (binding: BindingRef | undefined, ctx: CredentialCtx) => Promise<ApiCredential | undefined> | ApiCredential | undefined;

interface Scope { binding?: { id: string }; credential?: { apiKey: string } }
// Looked up lazily: the host creates the store on first use, possibly after this module loaded.
const scope = (): Scope | undefined => (globalThis as unknown as Record<symbol, AsyncLocalStorage<Scope> | undefined>)[Symbol.for("allternit.aai.callScope")]?.getStore();

/** Current call's binding (if the host set one). */
export const currentBinding = (): BindingRef | undefined => (scope()?.binding ? { id: scope()!.binding!.id } : undefined);

/** Default resolver: the short-lived credential allternit-api attached to this /aai/call request. */
export const callScopeCredentialResolver: ResolveCredential = () => {
  const c = scope()?.credential;
  return c?.apiKey ? { apiKey: c.apiKey } : undefined;
};

export const fingerprint = (apiKey: string): string => createHash("sha256").update(apiKey).digest("hex").slice(0, 16);

/** Removes any occurrence of the key from text that may leave the adapter (errors, logs). */
export const redact = (text: string, apiKey?: string): string => {
  let out = text;
  if (apiKey) out = out.split(apiKey).join("[redacted]");
  return out.replace(/sk-ant-[A-Za-z0-9_-]{6,}/g, "[redacted]");
};
