// DecisionReadoutProvider: logits + calibrated readouts first. A provider turns a
// request + state into a probability vector over the request's options and says
// which readout kind produced it. Providers never decide actions; the router does.
import type { DecisionRequestV1, CalibrationScope } from "./contract.ts";
import { applyTemperature } from "./metrics.ts";

export type ReadoutKind = "RAW_LOGIT" | "DEBIASED_LOGIT" | "CALIBRATED_LOGIT";

export interface Readout {
  /** Option ids, aligned with probs. */
  options: string[];
  probs: number[];
  kind: ReadoutKind;
  /** "categorical": probs sum to 1 (CHOICE/RANK/SCORE/ESTIMATE/PAIR_SCORE). "independent": one P(yes) per option (SUBSET). */
  shape?: "categorical" | "independent";
  /** How the distribution was obtained (logprobs are real logits; sampled is a vote estimate). */
  method: "logprobs" | "sampled" | "remote" | "fixture";
  latency_ms: number;
  /** The exact deployment scope this provider is running, minus question/candidate binding. */
  deployment: Omit<CalibrationScope, "question_id" | "candidate_schema_hash" | "candidate_set_hash" | "threshold_profile">;
  usage?: { input_tokens: number; output_tokens: number };
}

export interface DecisionReadoutProvider {
  readonly backend_id: string;
  readout(req: DecisionRequestV1, state: string): Promise<Readout>;
}

/** Wrap a raw readout with a manifest-provided temperature (kind becomes CALIBRATED_LOGIT). */
export function calibrateReadout(r: Readout, temperature: number): Readout {
  return { ...r, probs: calibrateProbs(r.probs, temperature, r.shape), kind: "CALIBRATED_LOGIT" };
}

/** Temperature scaling: softmax for categorical readouts, per-option sigmoid(logit/T) for independent ones. */
export function calibrateProbs(probs: number[], T: number, shape: Readout["shape"] = "categorical"): number[] {
  if (shape !== "independent") return applyTemperature(probs, T);
  return probs.map((p) => {
    const c = Math.min(Math.max(p, 1e-12), 1 - 1e-12);
    return 1 / (1 + Math.exp(-Math.log(c / (1 - c)) / T));
  });
}

/** Turn a probability vector into the operation's answer and a confidence (P that the answer is right). */
export function shapeAnswer(op: DecisionRequestV1["operation"], options: string[], probs: number[], scale?: string[]): { answer: unknown; confidence: number } {
  const order = probs.map((p, i) => i).sort((a, b) => probs[b] - probs[a]);
  switch (op) {
    case "RANK": return { answer: order.map((i) => options[i]), confidence: probs[order[0]] };
    case "SUBSET": return { answer: options.filter((_, i) => probs[i] >= 0.5), confidence: probs.length ? Math.min(...probs.map((p) => Math.max(p, 1 - p))) : 1 };
    case "ESTIMATE": {
      const mean = probs.reduce((a, p, i) => a + i * p, 0);
      return { answer: { value: mean, level: scale?.[Math.round(mean)] ?? String(Math.round(mean)) }, confidence: probs[order[0]] };
    }
    default: return { answer: options[order[0]], confidence: probs[order[0]] };
  }
}

/** Options a request exposes: BELIEF/GATE/VERIFY are yes/no, SCORE uses `scale`, else candidates. */
export function optionsOf(req: DecisionRequestV1): string[] {
  switch (req.operation) {
    case "BELIEF": case "GATE": case "VERIFY": return ["true", "false"];
    case "SCORE": case "ESTIMATE": return (req.scale ?? []).map((_, i) => String(i));
    default: return (req.candidates ?? []).map((c) => c.candidate_id);
  }
}

/** Static provider for tests, offline evals and replay. */
export class FixtureReadoutProvider implements DecisionReadoutProvider {
  constructor(
    readonly backend_id: string,
    private probs: number[],
    private deployment: Readout["deployment"],
    private kind: ReadoutKind = "RAW_LOGIT",
  ) {}
  async readout(req: DecisionRequestV1): Promise<Readout> {
    return { options: optionsOf(req), probs: this.probs, kind: this.kind, method: "fixture", latency_ms: 0, deployment: this.deployment };
  }
}
