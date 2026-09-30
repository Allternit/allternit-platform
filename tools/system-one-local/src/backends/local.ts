// `local` backend (default). Each question is evaluated independently against
// the same state: one constrained single-token labelling call per question,
// probabilities read from the next-token top-k logprobs and renormalized over
// the valid labels. No free text is ever parsed as a probability.
//
// Fallback: if the runtime returns no logprobs (or none of the valid labels are
// in the top-k), the question is answered by k-sample voting at temperature 1
// and reported as method "sampled" in `x_allternit.methods`.
import {
  confidence, labelDistribution, roundDistribution, round, voteDistribution, weightedScore,
} from "../math.ts";
import {
  MAX_LABELS_PER_CALL, choiceTask, firstToken, groupTask, noulTask, render, scoreTask, type LabelTask,
} from "../prompt.ts";
import {
  SystemOneError, type Answer, type Method, type Question, type Structured, type SystemOneRequest,
  type SystemOneResponse,
} from "../types.ts";
import { RuntimeUnavailable, type ChatRuntime } from "./runtime.ts";

export interface LocalBackendOptions {
  runtime: ChatRuntime;
  /** Parallel questions per request. Ollama serialises unless OLLAMA_NUM_PARALLEL > 1. */
  concurrency?: number;
  /** Samples per question for the no-logprobs fallback. */
  samples?: number;
  /** Per-call timeout. */
  timeoutMs?: number;
  /**
   * Position-bias debiasing: score every question twice (options forward and
   * reversed) and average. Doubles calls. Default off.
   */
  debias?: boolean;
}

/** Model names the local backend accepts. `jev-*` aliases are accepted so official-SDK clients work unchanged. */
export const LOCAL_ALIASES = new Set(["local", "local-latest", "jev-latest", "jev-preview", "default"]);

interface Dist {
  probs: number[];
  method: Method;
  labelMass?: number;
}

export class LocalBackend {
  readonly name = "local";
  private concurrency: number;
  private samples: number;
  private timeoutMs: number;
  private debias: boolean;
  constructor(private opts: LocalBackendOptions) {
    this.concurrency = Math.max(1, opts.concurrency ?? 4);
    this.samples = Math.max(2, opts.samples ?? 8);
    this.timeoutMs = opts.timeoutMs ?? 60_000;
    this.debias = opts.debias ?? false;
  }

  get model() {
    return this.opts.runtime.model;
  }

  resolveModel(requested: string): string {
    if (LOCAL_ALIASES.has(requested) || /^jev-\d/.test(requested)) return this.opts.runtime.model;
    if (requested.startsWith("local:") && requested.slice(6) === this.opts.runtime.model) return this.opts.runtime.model;
    throw new SystemOneError(422, {
      error: {
        type: "invalid_request_error",
        message: `unknown model "${requested}" for the local backend; use one of ${[...LOCAL_ALIASES].join(", ")} or local:${this.opts.runtime.model}`,
        details: [{ path: "model", message: "unknown model" }],
      },
    });
  }

  async evaluate(req: SystemOneRequest): Promise<SystemOneResponse> {
    this.resolveModel(req.model);
    const t0 = performance.now();
    const usage = { input: 0, output: 0 };
    const answers: Record<string, Answer> = {};
    const methods: Record<string, Method> = {};
    const labelMass: Record<string, number> = {};
    const ids = Object.keys(req.questions);
    let next = 0;
    const worker = async () => {
      while (next < ids.length) {
        const id = ids[next++];
        const q = req.questions[id];
        const { answer, dist } = await this.answer(req.state, q, usage);
        answers[id] = answer;
        methods[id] = dist.method;
        if (dist.labelMass !== undefined) labelMass[id] = round(dist.labelMass);
      }
    };
    try {
      await Promise.all(Array.from({ length: Math.min(this.concurrency, ids.length) }, worker));
    } catch (e) {
      if (e instanceof RuntimeUnavailable) {
        throw new SystemOneError(529, { error: { type: "overloaded_error", message: e.message } });
      }
      throw e;
    }
    // Keep answer order identical to question order.
    const ordered: Record<string, Answer> = {};
    for (const id of ids) ordered[id] = answers[id];
    return {
      model: `allternit-local/${this.opts.runtime.model}`,
      answers: ordered,
      usage: { input_tokens: usage.input, output_tokens: usage.output },
      x_allternit: {
        backend: "local",
        runtime: this.opts.runtime.name,
        methods,
        label_mass: labelMass,
        latency_ms: Math.round(performance.now() - t0),
      },
    };
  }

  private async answer(state: Structured, q: Question, usage: { input: number; output: number }) {
    switch (q.type) {
      case "noul": {
        const dist = await this.distribution((r) => noulTask(state, q.instructions, q.criteria, r), usage);
        return { answer: { type: "noul", noul: round(dist.probs[0]) } as Answer, dist };
      }
      case "score": {
        const dist = await this.distribution((r) => scoreTask(state, q.instructions, q.criteria, r), usage);
        const p = roundDistribution(dist.probs);
        const legend: Record<string, string> = {};
        const probabilities: Record<string, number> = {};
        q.criteria.forEach((lvl, i) => {
          legend[String(i)] = render(lvl);
          probabilities[String(i)] = p[i];
        });
        return {
          answer: {
            type: "score", score: round(weightedScore(dist.probs)), legend, probabilities,
            confidence: round(confidence(dist.probs)),
          } as Answer,
          dist,
        };
      }
      case "choice": {
        const options = Object.entries(q.criteria);
        const dist = options.length <= MAX_LABELS_PER_CALL
          ? await this.distribution((r) => choiceTask(state, q.instructions, options, r), usage)
          : await this.hierarchical(state, q.instructions, options, usage);
        const p = roundDistribution(dist.probs);
        const probabilities: Record<string, number> = {};
        options.forEach(([name], i) => (probabilities[name] = p[i]));
        let best = 0;
        dist.probs.forEach((v, i) => { if (v > dist.probs[best]) best = i; });
        return {
          answer: {
            type: "choice", choice: options[best][0], probabilities, confidence: round(confidence(dist.probs)),
          } as Answer,
          dist,
        };
      }
    }
  }

  /**
   * >20 options (top_logprobs cap): split into ≤20 groups of ≤20, score the group,
   * then score within every group; P(option) = P(group) · P(option | group).
   */
  private async hierarchical(
    state: Structured, instructions: Structured, options: [string, Structured | null][],
    usage: { input: number; output: number },
  ): Promise<Dist> {
    const size = MAX_LABELS_PER_CALL;
    const groups: [string, Structured | null][][] = [];
    for (let i = 0; i < options.length; i += size) groups.push(options.slice(i, i + size));
    const g = await this.distribution((r) => groupTask(state, instructions, groups, r), usage);
    const probs: number[] = [];
    let method: Method = g.method;
    for (let gi = 0; gi < groups.length; gi++) {
      const grp = groups[gi];
      const inner = grp.length === 1
        ? { probs: [1], method: "logprobs" as Method }
        : await this.distribution((r) => choiceTask(state, instructions, grp, r), usage);
      if (inner.method === "sampled") method = "sampled";
      for (const p of inner.probs) probs.push(g.probs[gi] * p);
    }
    return { probs, method };
  }

  private async distribution(build: (reverse: boolean) => LabelTask, usage: { input: number; output: number }): Promise<Dist> {
    const fwd = await this.once(build(false), usage);
    if (!this.debias) return fwd;
    const rev = await this.once(build(true), usage);
    return {
      probs: fwd.probs.map((p, i) => (p + rev.probs[i]) / 2),
      method: fwd.method === "sampled" || rev.method === "sampled" ? "sampled" : "logprobs",
      labelMass: fwd.labelMass !== undefined && rev.labelMass !== undefined ? (fwd.labelMass + rev.labelMass) / 2 : undefined,
    };
  }

  private async once(task: LabelTask, usage: { input: number; output: number }): Promise<Dist> {
    const r = await this.opts.runtime.complete(
      { messages: task.messages, maxTokens: 1, temperature: 0, topLogprobs: MAX_LABELS_PER_CALL },
      AbortSignal.timeout(this.timeoutMs),
    );
    usage.input += r.usage.input;
    usage.output += r.usage.output;
    if (r.top) {
      const d = labelDistribution(r.top, task.labels, { caseInsensitive: task.caseInsensitive });
      if (d) return { probs: d.probs, method: "logprobs", labelMass: d.labelMass };
    }
    // Fallback: k-sample voting on the constrained label.
    const samples: (string | null)[] = [];
    for (let i = 0; i < this.samples; i++) {
      const s = await this.opts.runtime.complete(
        { messages: task.messages, maxTokens: 3, temperature: 1, seed: 1000 + i },
        AbortSignal.timeout(this.timeoutMs),
      );
      usage.input += s.usage.input;
      usage.output += s.usage.output;
      samples.push(firstToken(s.text));
    }
    return { probs: voteDistribution(samples, task.labels, { caseInsensitive: task.caseInsensitive }), method: "sampled" };
  }
}
