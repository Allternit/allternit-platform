// Backend selection + the library entry point.
import { LocalBackend, LOCAL_ALIASES } from "./backends/local.ts";
import { OpenAICompatRuntime, type ChatRuntime, type FetchLike } from "./backends/runtime.ts";
import { LAYA_PREFIX, LAYA_URL, TypeSafeBackend, TYPESAFE_PREFIX } from "./backends/typesafe.ts";
import { logCall } from "./log.ts";
import { SystemOneError, type SystemOneRequest, type SystemOneResponse } from "./types.ts";
import { validateRequest } from "./validate.ts";

export interface EngineConfig {
  /** OpenAI-compatible base URL of the local runtime. Default: Ollama. */
  runtimeUrl: string;
  runtimeModel: string;
  concurrency: number;
  samples: number;
  /** Average forward + reversed option order (2× calls). SYSTEM_ONE_DEBIAS=1. */
  debias: boolean;
  typesafeKey?: string;
  /** Local Laya server (`laya.serve`); requests with model "laya:<checkpoint>" go there. */
  layaUrl: string;
  layaModel: string;
  logEnabled: boolean;
}

export function configFromEnv(env: Record<string, string | undefined> = process.env): EngineConfig {
  return {
    runtimeUrl: env.SYSTEM_ONE_RUNTIME_URL ?? "http://127.0.0.1:11434/v1",
    runtimeModel: env.SYSTEM_ONE_RUNTIME_MODEL ?? "llama3.2:latest",
    concurrency: Number(env.SYSTEM_ONE_CONCURRENCY ?? 4),
    samples: Number(env.SYSTEM_ONE_SAMPLES ?? 8),
    debias: env.SYSTEM_ONE_DEBIAS === "1",
    typesafeKey: env.TYPESAFE_API_KEY || undefined,
    layaUrl: (env.SYSTEM_ONE_LAYA_URL || LAYA_URL).replace(/\/$/, ""),
    layaModel: env.SYSTEM_ONE_LAYA_MODEL || "typed-decisions",
    logEnabled: env.SYSTEM_ONE_LOG === "1",
  };
}

export class SystemOne {
  readonly local: LocalBackend;
  readonly typesafe?: TypeSafeBackend;
  readonly laya: TypeSafeBackend;
  constructor(
    readonly config: EngineConfig = configFromEnv(),
    deps: { runtime?: ChatRuntime; fetchImpl?: FetchLike } = {},
  ) {
    const runtime = deps.runtime ?? new OpenAICompatRuntime(config.runtimeUrl, config.runtimeModel, deps.fetchImpl);
    this.local = new LocalBackend({ runtime, concurrency: config.concurrency, samples: config.samples, debias: config.debias });
    if (config.typesafeKey) this.typesafe = new TypeSafeBackend(config.typesafeKey, deps.fetchImpl);
    this.laya = new TypeSafeBackend(undefined, deps.fetchImpl, `${config.layaUrl}/v1/systemone`, "laya", LAYA_PREFIX);
  }

  /** Validate, route to a backend, evaluate. Throws SystemOneError on 4xx/5xx conditions. */
  async evaluate(body: unknown): Promise<SystemOneResponse> {
    const req = validateRequest(body);
    const res = await this.route(req).evaluate(req);
    logCall(req, res, this.config.logEnabled);
    return res;
  }

  private route(req: SystemOneRequest) {
    if (req.model.startsWith(LAYA_PREFIX)) return this.laya;
    if (req.model.startsWith(TYPESAFE_PREFIX)) {
      if (!this.typesafe) {
        throw new SystemOneError(401, {
          error: { type: "authentication_error", message: "typesafe backend requested but TYPESAFE_API_KEY is not set" },
        });
      }
      return this.typesafe;
    }
    return this.local;
  }

  models() {
    const data = [
      ...[...LOCAL_ALIASES].map((id) => ({
        id,
        description: `Local System One on ${this.local.model} (token logprobs over constrained labels)`,
        backend: "local",
      })),
      { id: `local:${this.local.model}`, description: "Pinned local runtime model", backend: "local" },
    ];
    data.push({ id: `${LAYA_PREFIX}${this.config.layaModel}`, description: `Laya on ${this.config.layaUrl} (local, ModernBERT, one forward pass)`, backend: "laya" });
    if (this.typesafe) {
      data.push({ id: `${TYPESAFE_PREFIX}jev-latest`, description: "Official TypeSafe Jev (remote, paid, passthrough)", backend: "typesafe" });
    }
    return { object: "list", data };
  }
}
