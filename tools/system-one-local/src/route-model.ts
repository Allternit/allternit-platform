// `route-model` helper. OpenRouter only offers `typesafe/jev-router`: a
// model-ROUTING chat model, not the System One API. It is deliberately NOT used
// to fake /v1/systemone answers. This helper asks it which model a task should
// go to and returns what OpenRouter reports it routed to.
//
// Paid call. Off unless the caller passes `allowPaid: true` (CLI: --allow-paid)
// and OPENROUTER_API_KEY is set.
import type { FetchLike } from "./backends/runtime.ts";

export const OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions";
export const JEV_ROUTER_MODEL = "typesafe/jev-router";

export interface RouteModelOptions {
  task: string;
  apiKey?: string;
  allowPaid?: boolean;
  fetchImpl?: FetchLike;
  maxTokens?: number;
}

export interface RouteModelResult {
  requested: string;
  /** Model OpenRouter says actually served the request (the router's pick). */
  routed_model: string | null;
  content: string;
  usage: { input_tokens: number; output_tokens: number };
}

export class RouteModelRefused extends Error {}

export async function routeModel(opts: RouteModelOptions): Promise<RouteModelResult> {
  if (!opts.allowPaid) throw new RouteModelRefused("route-model makes a paid OpenRouter call; pass allowPaid/--allow-paid to confirm");
  const key = opts.apiKey ?? process.env.OPENROUTER_API_KEY;
  if (!key) throw new RouteModelRefused("OPENROUTER_API_KEY is not set");
  if (!opts.task.trim()) throw new RouteModelRefused("task is empty");
  const f = opts.fetchImpl ?? fetch;
  const res = await f(OPENROUTER_URL, {
    method: "POST",
    headers: {
      authorization: `Bearer ${key}`,
      "content-type": "application/json",
      "x-title": "allternit-system-one route-model",
    },
    body: JSON.stringify({
      model: JEV_ROUTER_MODEL,
      messages: [{ role: "user", content: opts.task }],
      max_tokens: opts.maxTokens ?? 256,
    }),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`openrouter ${res.status}: ${text.slice(0, 300)}`);
  const data = JSON.parse(text);
  return {
    requested: JEV_ROUTER_MODEL,
    routed_model: typeof data?.model === "string" ? data.model : null,
    content: String(data?.choices?.[0]?.message?.content ?? ""),
    usage: {
      input_tokens: Number(data?.usage?.prompt_tokens ?? 0),
      output_tokens: Number(data?.usage?.completion_tokens ?? 0),
    },
  };
}
