// Per-call scope for POST /aai/call: the binding plus a short-lived, caller-supplied credential
// (allternit-api attaches `credential` next to `binding`). It lives only in AsyncLocalStorage for the
// duration of one call: never persisted, logged, or added to any event/error. Adapters that need a
// user-owned key read it through the same global symbol (adapters/claude-managed-agents/credential.ts),
// so no import coupling exists between src/ and adapters/.
import { AsyncLocalStorage } from "node:async_hooks";

export interface CallScope { binding?: { id: string; [k: string]: unknown }; credential?: { apiKey: string } }

const KEY = Symbol.for("allternit.aai.callScope");
const g = globalThis as unknown as Record<symbol, AsyncLocalStorage<CallScope> | undefined>;
const store = (g[KEY] ??= new AsyncLocalStorage<CallScope>());

export const runWithCallScope = <T>(scope: CallScope, fn: () => T): T => store.run(scope, fn);
export const getCallScope = (): CallScope | undefined => store.getStore();

/** Validates the optional request-body credential; anything malformed is treated as absent. */
export function parseCredential(raw: unknown): { apiKey: string } | undefined {
  if (!raw || typeof raw !== "object") return undefined;
  const k = (raw as { apiKey?: unknown }).apiKey;
  return typeof k === "string" && k.trim() !== "" ? { apiKey: k.trim() } : undefined;
}
