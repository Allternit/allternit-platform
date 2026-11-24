/**
 * Runtime registry for console agents (Harness & Bots 2.0, contract
 * CONTRACT-harness-bots-2.md: "Registry entry", "Compatibility",
 * "Backend gaps 1-3").
 *
 * An agent has a runtime (how it works: Allternit, Claude Code, Hermes, ...)
 * and a brain (the model it thinks with). The registry is cloud-api's public
 * `GET /api/v1/runtimes` (`{version, runtimes}`). The last good body is kept
 * in memory and in localStorage, so a reload offline still shows the list.
 * Until it loads (or when the route is not deployed yet) the console falls
 * back to Allternit only, which runs every brain.
 *
 * `fits` / `allowed` are the same rules gizzi, web, iOS and Android use; the
 * console is a separate app, so they are copied here, not imported.
 */

import { api } from "@/lib/api-client";

export type RuntimeCategory = "native" | "coding" | "personal" | "general" | "experimental";
export type AccessKind = "plan" | "key" | "local" | "credits";

export interface RuntimeEntry {
  id: string;
  name: string;
  vendor: string;
  category: RuntimeCategory;
  tool: string | null;
  aliases?: string[];
  toolAliases?: string[];
  brains: "any" | string[];
  access: AccessKind[];
  more: boolean;
  status?: { label: string; tone: "warn" | "info" };
  /** Subscription providers (claude, chatgpt, kimi, gemini, copilot) whose plan this runtime may use. */
  plans?: string[];
  docs?: string;
}

export interface RuntimesBody {
  version: number;
  runtimes: RuntimeEntry[];
}

export const DEFAULT_RUNTIME_ID = "allternit";

export const ALLTERNIT_RUNTIME: RuntimeEntry = {
  id: DEFAULT_RUNTIME_ID,
  name: "Allternit",
  vendor: "Allternit",
  category: "native",
  tool: null,
  brains: "any",
  access: ["credits", "key", "plan"],
  more: false,
  plans: ["claude", "chatgpt", "kimi", "gemini", "copilot"],
};

export const CATEGORY_LABEL: Record<RuntimeCategory, string> = {
  native: "Allternit",
  coding: "Coding",
  personal: "Personal",
  general: "General",
  experimental: "Experimental",
};

export const ACCESS_LABEL: Record<AccessKind, string> = {
  plan: "Plan",
  key: "API key",
  local: "Local",
  credits: "Credits",
};

// ─── Cache ──────────────────────────────────────────────────────────────────

const CACHE_KEY = "allternit.console.runtimes.v1";
let memory: RuntimesBody | null = null;
let inflight: Promise<RuntimesBody> | null = null;

function isRuntimesBody(value: unknown): value is RuntimesBody {
  if (!value || typeof value !== "object") return false;
  const v = value as { version?: unknown; runtimes?: unknown };
  return typeof v.version === "number" && Array.isArray(v.runtimes);
}

function readCache(): RuntimesBody | null {
  try {
    const raw = window.localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    return isRuntimesBody(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function writeCache(body: RuntimesBody): void {
  try {
    window.localStorage.setItem(CACHE_KEY, JSON.stringify(body));
  } catch {
    // Quota, private mode or blocked storage: the in-memory copy still works.
  }
}

/** The registry right now without a request: memory, then localStorage, else null. */
export function cachedRuntimes(): RuntimeEntry[] | null {
  if (!memory) memory = readCache();
  return memory ? withAllternit(memory.runtimes) : null;
}

/** Allternit always comes first, even when an old body lacks it. */
function withAllternit(list: RuntimeEntry[]): RuntimeEntry[] {
  const rest = list.filter((r) => r.id !== DEFAULT_RUNTIME_ID);
  const own = list.find((r) => r.id === DEFAULT_RUNTIME_ID);
  return [own ? { ...ALLTERNIT_RUNTIME, ...own } : ALLTERNIT_RUNTIME, ...rest];
}

/**
 * Fetch the registry from cloud-api. On failure the cached copy is returned;
 * with no cache the error is thrown and callers show Allternit only.
 */
export async function fetchRuntimes(): Promise<RuntimeEntry[]> {
  if (!inflight) {
    inflight = api
      .get<unknown>("/api/v1/runtimes")
      .then((body) => {
        if (!isRuntimesBody(body)) throw new Error("The runtime list came back in an unexpected shape.");
        memory = body;
        writeCache(body);
        return body;
      })
      .finally(() => {
        inflight = null;
      });
  }
  try {
    return withAllternit((await inflight).runtimes);
  } catch (err) {
    const cached = cachedRuntimes();
    if (cached) return cached;
    throw err;
  }
}

export function findRuntime(list: readonly RuntimeEntry[], id: string | null | undefined): RuntimeEntry | undefined {
  const key = (id || DEFAULT_RUNTIME_ID).toLowerCase();
  return list.find((r) => r.id === key || (r.aliases ?? []).includes(key));
}

// ─── Rules (shared rule: handoffs/harness-bots-2-2026-11-20/ALLOWED-RULE.md,
//     reference gizzi runtime-registry.ts / platform #2785) ──────────────────

/** Built-in `plans` table, used when the registry body has no `plans` yet (older server). */
const PLANS_FALLBACK: Record<string, string[]> = {
  allternit: ["claude", "chatgpt", "kimi", "gemini", "copilot"],
  claude: ["claude"],
  codex: ["chatgpt"],
  kimi: ["kimi"],
  antigravity: ["gemini"],
  copilot: ["copilot"],
};

export const PLAN_OF_VENDOR: Record<string, string> = {
  anthropic: "claude",
  openai: "chatgpt",
  google: "gemini",
  moonshot: "kimi",
  microsoft: "copilot",
};

export const PLAN_NAME: Record<string, string> = {
  claude: "Claude",
  chatgpt: "ChatGPT",
  kimi: "Kimi",
  gemini: "Gemini",
  copilot: "Copilot",
};

const VENDOR_OF_PLAN: Record<string, string> = Object.fromEntries(
  Object.entries(PLAN_OF_VENDOR).map(([vendor, plan]) => [plan, vendor])
);

/** The plans a runtime may use: the registry's `plans`, else the built-in table. */
export function plansOf(runtime: RuntimeEntry): string[] {
  if (Array.isArray(runtime.plans)) return runtime.plans;
  return PLANS_FALLBACK[runtime.id] ?? [];
}

/** Plan id a vendor (or plan id) has, for the console's Access › plan button. */
export function planOf(vendorOrPlan: string): string | undefined {
  const v = vendorOrPlan.toLowerCase();
  return PLAN_OF_VENDOR[v] ?? (PLAN_NAME[v] ? v : undefined);
}

/**
 * The plan a provider id signs in with, or undefined (API key, local, credits):
 * `subs-<rest>` → PLAN_OF_VENDOR[rest] ?? rest; a CLI plan sign-in (the
 * runtime whose `tool` is the id and has exactly one plan) → that plan.
 */
export function planProvider(providerId: string, runtimes: readonly RuntimeEntry[] = []): string | undefined {
  if (providerId.startsWith("subs-")) {
    const rest = providerId.slice(5);
    return PLAN_OF_VENDOR[rest] ?? rest;
  }
  const owner = runtimes.find((r) => r.tool === providerId && plansOf(r).length === 1);
  return owner ? plansOf(owner)[0] : undefined;
}

/** Provider id equals the runtime's tool, id, one of its aliases, or one of the tool's aliases. */
export function isOwnCli(runtime: RuntimeEntry, providerId: string): boolean {
  if (providerId === runtime.id) return true;
  if (!runtime.tool) return false;
  return (
    providerId === runtime.tool ||
    (runtime.aliases ?? []).includes(providerId) ||
    (runtime.toolAliases ?? []).includes(providerId)
  );
}

/** The provider id sent for a brain: `subs-<vendor>` when it runs on a plan. */
export function providerFor(vendorProvider: string, access: AccessKind): string {
  if (access !== "plan") return vendorProvider;
  const plan = planOf(vendorProvider);
  return plan ? `subs-${VENDOR_OF_PLAN[plan] ?? plan}` : vendorProvider;
}

/** Does a brain from `providerId` fit this runtime? (contract "Compatibility: provider ids") */
export function fits(runtime: RuntimeEntry, providerId: string): boolean {
  if (runtime.brains === "any") return true;
  const p = providerId.replace(/^subs-/, "");
  if (runtime.brains.includes(p)) return true;
  if (!runtime.tool) return false;
  return isOwnCli(runtime, p);
}

/** fits AND (not a plan, or the runtime's own CLI, or the plan is in `runtime.plans`). */
export function allowed(runtime: RuntimeEntry, providerId: string, runtimes: readonly RuntimeEntry[] = []): boolean {
  if (!fits(runtime, providerId)) return false;
  const plan = planProvider(providerId, runtimes);
  return plan === undefined || isOwnCli(runtime, providerId) || plansOf(runtime).includes(plan);
}

/** The first brain (in the caller's order) that `allowed` accepts. */
export function firstAllowedBrain<T>(
  runtime: RuntimeEntry,
  brains: readonly T[],
  providerOf: (brain: T) => string,
  runtimes: readonly RuntimeEntry[] = []
): T | undefined {
  return brains.find((b) => allowed(runtime, providerOf(b), runtimes));
}

/**
 * Same text the server sends for `plan_not_allowed_on_runtime` (only when the
 * brain fits but is not allowed); null otherwise.
 */
export function planRefusal(
  runtimes: readonly RuntimeEntry[],
  runtime: RuntimeEntry,
  providerId: string
): string | null {
  if (!fits(runtime, providerId) || allowed(runtime, providerId, runtimes)) return null;
  const plan = planProvider(providerId, runtimes);
  if (!plan) return null;
  const own = runtimes.find((r) => r.id !== DEFAULT_RUNTIME_ID && plansOf(r).includes(plan));
  return `${PLAN_NAME[plan] ?? plan}'s plan can't be used inside ${runtime.name}. Use an API key, or pick ${own?.name ?? "Allternit"}.`;
}

/** Field errors the server returns as 422 `{error, message}`. */
export const RUNTIME_ERROR_CODES = ["runtime_unknown", "brain_not_fit", "plan_not_allowed_on_runtime"] as const;
export type RuntimeErrorCode = (typeof RUNTIME_ERROR_CODES)[number];
export function isRuntimeErrorCode(code: unknown): code is RuntimeErrorCode {
  return typeof code === "string" && (RUNTIME_ERROR_CODES as readonly string[]).includes(code);
}
