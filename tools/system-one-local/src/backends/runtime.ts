// Thin client for an OpenAI-compatible local runtime: Ollama (/v1, default
// http://127.0.0.1:11434/v1) or llama-server (/v1, e.g. http://127.0.0.1:8080/v1).
// Both return `logprobs.content[0].top_logprobs` for chat completions (top_logprobs ≤ 20).
import type { ChatMessage } from "../prompt.ts";
import type { TopLogprob } from "../math.ts";

export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

export interface CompletionRequest {
  messages: ChatMessage[];
  maxTokens: number;
  temperature: number;
  topLogprobs?: number; // omit → no logprobs requested
  seed?: number;
}

export interface CompletionResult {
  text: string;
  /** Top-k for the FIRST generated token, or null when the runtime returned no logprobs. */
  top: TopLogprob[] | null;
  usage: { input: number; output: number };
}

export interface ChatRuntime {
  readonly name: string;
  readonly model: string;
  complete(req: CompletionRequest, signal?: AbortSignal): Promise<CompletionResult>;
}

export class RuntimeUnavailable extends Error {}

export class OpenAICompatRuntime implements ChatRuntime {
  readonly name: string;
  constructor(
    readonly baseUrl: string,
    readonly model: string,
    private fetchImpl: FetchLike = fetch,
  ) {
    this.name = baseUrl.includes(":11434") ? "ollama" : "openai-compat";
  }

  async complete(req: CompletionRequest, signal?: AbortSignal): Promise<CompletionResult> {
    const body: Record<string, unknown> = {
      model: this.model,
      messages: req.messages,
      max_tokens: req.maxTokens,
      temperature: req.temperature,
      stream: false,
    };
    if (req.topLogprobs) {
      body.logprobs = true;
      body.top_logprobs = req.topLogprobs;
      // llama-server also honours n_probs; harmless elsewhere.
      body.n_probs = req.topLogprobs;
    }
    if (req.seed !== undefined) body.seed = req.seed;
    let res: Response;
    try {
      res = await this.fetchImpl(`${this.baseUrl.replace(/\/$/, "")}/chat/completions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
        signal,
      });
    } catch (e) {
      throw new RuntimeUnavailable(`local runtime unreachable at ${this.baseUrl}: ${(e as Error).message}`);
    }
    if (!res.ok) {
      const txt = (await res.text()).slice(0, 300);
      throw new RuntimeUnavailable(`local runtime ${res.status}: ${txt}`);
    }
    const data = (await res.json()) as any;
    const choice = data?.choices?.[0];
    const first = choice?.logprobs?.content?.[0];
    const top: TopLogprob[] | null = Array.isArray(first?.top_logprobs) && first.top_logprobs.length
      ? first.top_logprobs.map((t: any) => ({ token: String(t.token), logprob: Number(t.logprob) }))
      : null;
    return {
      text: String(choice?.message?.content ?? ""),
      top,
      usage: {
        input: Number(data?.usage?.prompt_tokens ?? 0),
        output: Number(data?.usage?.completion_tokens ?? 0),
      },
    };
  }
}
