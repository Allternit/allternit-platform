/**
 * Person-only models: a plan the person connected answers only messages the
 * person sends in chat (CONTRACT-connections-2026-11 §9.3, §9.5, §9.9).
 *
 * Three local providers carry them:
 *   - `plan`         → cloud-api runner rows, model ids `plan/<vendor>/<model>`
 *   - `copilot-plan` → cloud-api Copilot route, model ids `copilot/<model>` on
 *                      the wire (the local id differs because `copilot/…` is
 *                      already the GitHub Copilot CLI brain in gizzi's catalog)
 *   - `chatgpt`      → ChatGPT direct sign-in on this computer, `chatgpt/<model>`
 *
 * Every other place a model runs (headless `gizzi run`, routines and cron,
 * missions, sub-agents, bots, title/summary helpers) refuses them with the
 * plain messages below. Only a composer send marks a turn as the person's:
 * the TUI main thread, or a chat bridge that puts `metadata.turn: "person"`
 * on the user message (Desktop's composer).
 */
import fs from "fs"
import os from "os"
import path from "path"

export const PLAN_PROVIDER = "plan"
export const COPILOT_PROVIDER = "copilot-plan"
export const CHATGPT_PROVIDER = "chatgpt"

const PERSON_ONLY = new Set([PLAN_PROVIDER, COPILOT_PROVIDER, CHATGPT_PROVIDER])

/** Header cloud-api requires on a plan turn (D7). */
export const PERSON_TURN_HEADER = "x-allternit-turn"
/** User-message metadata a chat bridge sets on a composer send. */
export const PERSON_TURN_METADATA = "turn"

export function isPersonOnlyProvider(providerID: string | undefined | null): boolean {
  return !!providerID && PERSON_ONLY.has(providerID)
}

/** `plan/claude/sonnet`, `copilot-plan/gpt-5`, `chatgpt/gpt-6.1-sol` → true. */
export function isPersonOnlyRef(ref: string | undefined | null): boolean {
  if (!ref) return false
  const slash = ref.indexOf("/")
  if (slash <= 0) return false
  return isPersonOnlyProvider(ref.slice(0, slash))
}

/** The id cloud-api expects for a local plan model: `plan/claude/sonnet`, `copilot/gpt-5`. */
export function wireModelID(providerID: string, modelID: string): string {
  if (providerID === COPILOT_PROVIDER) return `copilot/${modelID}`
  return `${providerID}/${modelID}`
}

/** True when a user message's metadata marks it as the person's own composer send. */
export function isPersonTurn(metadata: Record<string, unknown> | undefined | null): boolean {
  return metadata?.[PERSON_TURN_METADATA] === "person"
}

export type RunKind = "headless" | "routine" | "mission" | "subagent" | "bot" | "helper"

const WHERE: Record<RunKind, string> = {
  headless: "`gizzi run`",
  routine: "routines and schedules",
  mission: "missions",
  subagent: "sub-agents",
  bot: "bots",
  helper: "bots, routines, missions, sub-agents, headless runs and background helpers",
}

/** The plain refusal for a person-only model used where nobody is sending. */
export function refusal(ref: string, kind: RunKind): string {
  return (
    `${ref} is your own plan. It answers only messages you send yourself in the chat, so ${WHERE[kind]} cannot use it. ` +
    "Pick another model (for example an Allternit Cloud model or a CLI brain)."
  )
}

/** Undefined when the ref may run here, else the refusal. */
export function refuseIfPersonOnly(ref: string | undefined | null, kind: RunKind): string | undefined {
  return ref && isPersonOnlyRef(ref) ? refusal(ref, kind) : undefined
}

export const NEEDS_PERSON_SIGN_IN =
  "Plans answer only messages you send while signed in as yourself. This terminal is signed in to Allternit as a device " +
  "(`gizzi login`) or with an API key, and plans accept neither. Send plan messages from Allternit on the web, Desktop " +
  "or the iOS app. To chat with your ChatGPT plan here, run `gizzi connect chatgpt`."

function jwtClaims(token: string): Record<string, unknown> | undefined {
  const parts = token.split(".")
  if (parts.length !== 3) return undefined
  try {
    return JSON.parse(Buffer.from(parts[1]!, "base64url").toString("utf8"))
  } catch {
    return undefined
  }
}

/**
 * The person's own Allternit session token (a Clerk JWT in session.json), or
 * undefined. A device token (`gizzi login` / `gizzi pair`), an API key or an
 * expired session never counts: cloud-api refuses those for plan turns.
 */
export function personToken(now = Date.now()): string | undefined {
  try {
    const dir = process.env.GIZZI_CONFIG_DIR ?? path.join(os.homedir(), ".config", "gizzi-code")
    const session = JSON.parse(fs.readFileSync(path.join(dir, "session.json"), "utf8")) as {
      accessToken?: unknown
      expiresAt?: unknown
    }
    const token = typeof session.accessToken === "string" ? session.accessToken.trim() : ""
    if (!token) return undefined
    const claims = jwtClaims(token)
    if (!claims) return undefined
    const exp = typeof claims.exp === "number" ? claims.exp * 1000 : typeof session.expiresAt === "number" ? session.expiresAt : undefined
    if (exp !== undefined && exp <= now) return undefined
    return token
  } catch {
    return undefined
  }
}
