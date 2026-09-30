// Readout provider backed by the local typed-decision engine (token logprobs over
// constrained labels). This is the canonical S1 path: everything else is an adapter.
import type { SystemOne } from "../engine.ts";
import type { Question } from "../types.ts";
import type { DecisionRequestV1 } from "./contract.ts";
import { optionsOf, type DecisionReadoutProvider, type Readout } from "./readout.ts";

export interface LocalDeployment {
  model_ref: string; model_revision: string; tokenizer_id: string; quantization: string; runtime_backend: string;
}

export class LocalLogitReadoutProvider implements DecisionReadoutProvider {
  readonly backend_id: string;
  constructor(private engine: SystemOne, private dep: LocalDeployment, backendId = "backend.local_logit") {
    this.backend_id = backendId;
  }

  async readout(req: DecisionRequestV1, state: string): Promise<Readout> {
    const options = optionsOf(req);
    let q: Question;
    if (req.operation === "BELIEF" || req.operation === "GATE" || req.operation === "VERIFY") q = { type: "noul", instructions: req.instructions };
    else if (req.operation === "SCORE") q = { type: "score", instructions: req.instructions, criteria: req.scale ?? [] };
    else if (req.operation === "CHOICE") {
      q = { type: "choice", instructions: req.instructions, criteria: Object.fromEntries((req.candidates ?? []).map((c) => [c.candidate_id, c.label ?? c.candidate_id])) };
    } else throw new Error(`operation ${req.operation} is not served by the logit provider yet`);
    const res = await this.engine.evaluate({ model: "local", state, questions: { q } });
    const a = res.answers.q;
    let probs: number[];
    if (a.type === "noul") probs = [a.noul, 1 - a.noul];
    else probs = options.map((o) => (a.probabilities as Record<string, number>)[o] ?? 0);
    const method = res.x_allternit?.methods.q ?? "sampled";
    return {
      options, probs, kind: "RAW_LOGIT", method: method === "logprobs" ? "logprobs" : method === "remote" ? "remote" : "sampled",
      latency_ms: res.x_allternit?.latency_ms ?? 0, deployment: { backend_id: this.backend_id, ...this.dep },
      usage: res.usage,
    };
  }
}
