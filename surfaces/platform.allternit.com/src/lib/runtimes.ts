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

// ─── Rules ──────────────────────────────────────────────────────────────────

/**
 * Plan id (registry `plans` value) for a provider id: a vendor id, a
 * subscription id (`subs-anthropic`) or a CLI sign-in (`claude-cli`).
 */
const PLAN_OF: Record<string, string> = {
  anthropic: "claude",
  claude: "claude",
  "claude-cli": "claude",
  "claude-code": "claude",
  openai: "chatgpt",
  chatgpt: "chatgpt",
  codex: "chatgpt",
  "codex-cli": "chatgpt",
  moonshot: "kimi",
  moonshotai: "kimi",
  kimi: "kimi",
  "kimi-cli": "kimi",
  "kimi-code": "kimi",
  google: "gemini",
  gemini: "gemini",
  "gemini-cli": "gemini",
  antigravity: "gemini",
  agy: "gemini",
  copilot: "copilot",
  "github-copilot": "copilot",
  "copilot-cli": "copilot",
};

export const PLAN_NAME: Record<string, string> = {
  claude: "Claude",
  chatgpt: "ChatGPT",
  kimi: "Kimi",
  gemini: "Gemini",
  copilot: "Copilot",
};

/** The subscription vendor id used for a plan brain's provider (`subs-<vendor>`). */
const PLAN_VENDOR: Record<string, string> = {
  claude: "anthropic",
  chatgpt: "openai",
  kimi: "moonshot",
  gemini: "google",
  copilot: "github-copilot",
};

const CLI_PLAN_IDS = new Set(["claude-cli", "codex-cli", "kimi-cli", "gemini-cli", "copilot-cli", "agy", "antigravity"]);

function bare(providerId: string): string {
  return providerId.toLowerCase().replace(/^subs-/, "");
}

/** Plan id for a provider, or undefined when the provider has no consumer plan. */
export function planOf(providerId: string): string | undefined {
  return PLAN_OF[bare(providerId)];
}

/** True when a brain from this provider id is a plan brain (subscription or CLI plan sign-in). */
export function isPlanProvider(providerId: string): boolean {
  const p = providerId.toLowerCase();
  return p.startsWith("subs-") || CLI_PLAN_IDS.has(p);
}

/** The provider id sent for a brain: `subs-<vendor>` when it runs on a plan. */
export function providerFor(vendorProvider: string, access: AccessKind): string {
  if (access !== "plan") return vendorProvider;
  const plan = planOf(vendorProvider);
  return plan ? `subs-${PLAN_VENDOR[plan]}` : vendorProvider;
}

/** Ids a runtime's own CLI answers to (its tool, id and aliases). */
function ownIds(r: RuntimeEntry): string[] {
  if (!r.tool) return [];
  return [r.tool, r.id, ...(r.aliases ?? []), ...(r.toolAliases ?? [])].map((s) => s.toLowerCase());
}

/** Does a brain from `providerId` fit this runtime? (contract "Compatibility: provider ids") */
export function fits(runtime: RuntimeEntry, providerId: string): boolean {
  if (runtime.brains === "any") return true;
  const p = bare(providerId);
  if (runtime.brains.map((b) => b.toLowerCase()).includes(p)) return true;
  return ownIds(runtime).includes(p);
}

/** The plans a runtime may use, with the contract's fallback when the registry has no `plans`. */
export function plansOf(runtime: RuntimeEntry): string[] {
  if (Array.isArray(runtime.plans)) return runtime.plans;
  if (runtime.id === DEFAULT_RUNTIME_ID) return ALLTERNIT_RUNTIME.plans ?? [];
  const own = ownIds(runtime).map((id) => PLAN_OF[id]).filter((p): p is string => Boolean(p));
  return [...new Set(own)];
}

/**
 * fits AND, for a plan brain, the plan's provider is in `runtime.plans`.
 * A runtime's own CLI sign-in always passes.
 */
export function allowed(runtime: RuntimeEntry, providerId: string): boolean {
  if (!fits(runtime, providerId)) return false;
  if (!isPlanProvider(providerId)) return true;
  if (ownIds(runtime).includes(providerId.toLowerCase())) return true;
  const plan = planOf(providerId);
  return plan ? plansOf(runtime).includes(plan) : false;
}

/** The first brain (in the caller's order) that `allowed` accepts. */
export function firstAllowedBrain<T>(
  runtime: RuntimeEntry,
  brains: readonly T[],
  providerOf: (brain: T) => string
): T | undefined {
  return brains.find((b) => allowed(runtime, providerOf(b)));
}

/** The runtime that owns a plan (Claude plan → Claude Code), for the plan message. */
export function ownRuntimeForPlan(list: readonly RuntimeEntry[], plan: string): RuntimeEntry | undefined {
  return list.find((r) => r.id !== DEFAULT_RUNTIME_ID && r.tool && ownIds(r).some((id) => PLAN_OF[id] === plan));
}

/** Same text the server sends for `plan_not_allowed_on_runtime`. */
export function planNotAllowedMessage(list: readonly RuntimeEntry[], runtime: RuntimeEntry, plan: string): string {
  const own = ownRuntimeForPlan(list, plan);
  const pick = own ? `, or pick ${own.name}` : `, or pick ${ALLTERNIT_RUNTIME.name}`;
  return `${PLAN_NAME[plan] ?? plan}'s plan can't be used inside ${runtime.name}. Use an API key${pick}.`;
}

/** Field errors the server returns as 422 `{error, message}`. */
export const RUNTIME_ERROR_CODES = ["runtime_unknown", "brain_not_fit", "plan_not_allowed_on_runtime"] as const;
export type RuntimeErrorCode = (typeof RUNTIME_ERROR_CODES)[number];
export function isRuntimeErrorCode(code: unknown): code is RuntimeErrorCode {
  return typeof code === "string" && (RUNTIME_ERROR_CODES as readonly string[]).includes(code);
}
