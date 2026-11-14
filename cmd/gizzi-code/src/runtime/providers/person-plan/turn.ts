/**
 * One person-sent chat turn on a person-only model, no tools. Shared by the
 * TUI composer (ink-app personPlan.ts) and the engine's language model
 * (language-model.ts), so both send exactly the same request.
 *
 *   chatgpt/<model>       → POST https://api.openai.com/v1/responses (this computer's sign-in)
 *   plan/<vendor>/<model> → POST <cloud>/v1/chat/completions, X-Allternit-Turn: person
 *   copilot-plan/<model>  → same, model `copilot/<model>` on the wire
 *
 * Callers decide that the turn is the person's; this module only refuses
 * credentials that cannot carry one.
 */
import { cloudOrigin } from "@/runtime/providers/discovery/allternit-cloud"
import { Installation } from "@/shared/installation"
import { ChatgptAuthError, defaultDeps, usableSession, type Deps } from "./chatgpt-auth"
import { CHATGPT_PROVIDER, NEEDS_PERSON_SIGN_IN, PERSON_TURN_HEADER, personToken, wireModelID } from "./rules"
import {
  chatCompletionsBody,
  chatgptErrorText,
  parseChatCompletionsStream,
  parseResponsesStream,
  planErrorText,
  readErrorBody,
  responsesBody,
  type ChatMessage,
  type TurnEvent,
} from "./stream"

export const OPENAI_API = "https://api.openai.com/v1"

export type TurnInput = {
  providerID: string
  modelID: string
  messages: ChatMessage[]
  signal?: AbortSignal
  deps?: Partial<Deps>
}

export async function* personTurn(input: TurnInput): AsyncGenerator<TurnEvent> {
  if (input.providerID === CHATGPT_PROVIDER) {
    yield* chatgptTurn(input)
    return
  }
  yield* cloudPlanTurn(input)
}

async function* chatgptTurn(input: TurnInput): AsyncGenerator<TurnEvent> {
  const fetchImpl = input.deps?.fetch ?? fetch
  let token: string
  try {
    token = (await usableSession({ ...defaultDeps(), ...input.deps })).access_token
  } catch (e) {
    yield { type: "error", code: e instanceof ChatgptAuthError ? e.code : "signed_out", message: (e as Error).message }
    return
  }
  let res: Response
  try {
    res = await fetchImpl(`${OPENAI_API}/responses`, {
      method: "POST",
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json", accept: "text/event-stream" },
      body: JSON.stringify(responsesBody(input.modelID, input.messages)),
      signal: input.signal,
    })
  } catch (e) {
    if (input.signal?.aborted) throw e
    yield { type: "error", code: "network", message: `Could not reach ChatGPT: ${e instanceof Error ? e.message : String(e)}` }
    return
  }
  if (!res.ok || !res.body) {
    const err = readErrorBody(await res.text().catch(() => ""))
    yield { type: "error", code: err.code, message: chatgptErrorText(err.code, res.status, err.message) }
    return
  }
  yield* parseResponsesStream(res.body)
}

async function* cloudPlanTurn(input: TurnInput): AsyncGenerator<TurnEvent> {
  const token = personToken()
  if (!token) {
    yield { type: "error", code: "plan_person_only", message: NEEDS_PERSON_SIGN_IN }
    return
  }
  const fetchImpl = input.deps?.fetch ?? fetch
  let res: Response
  try {
    res = await fetchImpl(`${cloudOrigin()}/v1/chat/completions`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
        accept: "text/event-stream",
        [PERSON_TURN_HEADER]: "person",
        "x-allternit-client": `gizzi-code/${Installation.VERSION}`,
      },
      body: JSON.stringify(chatCompletionsBody(wireModelID(input.providerID, input.modelID), input.messages)),
      signal: input.signal,
    })
  } catch (e) {
    if (input.signal?.aborted) throw e
    yield { type: "error", code: "network", message: `Could not reach Allternit: ${e instanceof Error ? e.message : String(e)}` }
    return
  }
  if (!res.ok || !res.body) {
    const err = readErrorBody(await res.text().catch(() => ""))
    yield { type: "error", code: err.code, message: planErrorText(err.code, res.status, err.message) }
    return
  }
  yield* parseChatCompletionsStream(res.body)
}

/** `GET /v1/models` with the ChatGPT sign-in: display-ready models (`visibility == "list"`), server order. */
export function parseChatgptModels(body: any): { slug: string; name: string }[] {
  const rows: any[] = Array.isArray(body?.models) ? body.models : Array.isArray(body?.data) ? body.data : []
  const out: { slug: string; name: string }[] = []
  for (const r of rows) {
    const slug = typeof r?.slug === "string" ? r.slug : typeof r?.id === "string" ? r.id : undefined
    if (!slug) continue
    if (r.visibility !== undefined && r.visibility !== "list") continue
    out.push({ slug, name: typeof r.display_name === "string" && r.display_name ? r.display_name : slug })
  }
  return out
}

export async function fetchChatgptModels(accessToken: string, fetchImpl: typeof fetch = fetch): Promise<{ slug: string; name: string }[]> {
  const res = await fetchImpl(`${OPENAI_API}/models`, {
    headers: { authorization: `Bearer ${accessToken}` },
    signal: AbortSignal.timeout(8_000),
  })
  if (!res.ok) {
    const err = readErrorBody(await res.text().catch(() => ""))
    throw new ChatgptAuthError(chatgptErrorText(err.code, res.status, err.message), err.code ?? `http_${res.status}`)
  }
  return parseChatgptModels(await res.json())
}
