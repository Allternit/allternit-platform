// `typesafe` backend (optional, off unless TYPESAFE_API_KEY is set AND the
// request explicitly selects it with model "typesafe:<model>"). Pure
// passthrough to the official API: the body sent is exactly the caller's
// state + questions with the prefix stripped from `model`. Paid per input token.
import { SystemOneError, type SystemOneRequest, type SystemOneResponse } from "../types.ts";
import type { FetchLike } from "./runtime.ts";

export const TYPESAFE_URL = "https://api.typesafe.ai/v1/systemone";
export const TYPESAFE_PREFIX = "typesafe:";
/** Laya (convaiinnovations/laya, Apache-2.0) served locally by `laya.serve` speaks the same protocol. */
export const LAYA_PREFIX = "laya:";
export const LAYA_URL = "http://127.0.0.1:7718";

/**
 * A `/v1/systemone` passthrough. The official API needs a key; a local Laya
 * server (name "laya", no key) is the same protocol at another URL.
 */
export class TypeSafeBackend {
  constructor(
    private apiKey: string | undefined,
    private fetchImpl: FetchLike = fetch,
    private url = process.env.TYPESAFE_BASE_URL
      ? `${process.env.TYPESAFE_BASE_URL.replace(/\/$/, "")}/v1/systemone`
      : TYPESAFE_URL,
    readonly name: "typesafe" | "laya" = "typesafe",
    private prefix = TYPESAFE_PREFIX,
  ) {}

  async evaluate(req: SystemOneRequest): Promise<SystemOneResponse> {
    const t0 = performance.now();
    const model = req.model.startsWith(this.prefix) ? req.model.slice(this.prefix.length) : req.model;
    let res: Response;
    try {
      res = await this.fetchImpl(this.url, {
        method: "POST",
        headers: { ...(this.apiKey ? { authorization: `Bearer ${this.apiKey}` } : {}), "content-type": "application/json" },
        body: JSON.stringify({ model, state: req.state, questions: req.questions }),
      });
    } catch (e) {
      throw new SystemOneError(529, { error: { type: "overloaded_error", message: `${this.name} unreachable: ${(e as Error).message}` } });
    }
    const text = await res.text();
    let data: any;
    try {
      data = JSON.parse(text);
    } catch {
      data = null;
    }
    if (!res.ok) {
      const status = ([401, 422, 429, 529] as const).find((s) => s === res.status) ?? 500;
      const type = status === 401 ? "authentication_error" : status === 422 ? "invalid_request_error"
        : status === 429 ? "rate_limit_error" : status === 529 ? "overloaded_error" : "api_error";
      // Never echo the key; upstream body is truncated.
      throw new SystemOneError(status, { error: { type, message: `${this.name} ${res.status}: ${text.slice(0, 300)}` } });
    }
    if (!data?.answers) {
      throw new SystemOneError(500, { error: { type: "api_error", message: `${this.name} response missing answers` } });
    }
    const methods = Object.fromEntries(Object.keys(data.answers).map((k) => [k, "remote" as const]));
    return { ...data, x_allternit: { backend: this.name, methods, latency_ms: Math.round(performance.now() - t0) } };
  }
}
