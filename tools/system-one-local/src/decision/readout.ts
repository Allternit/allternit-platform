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
  return { ...r, probs: applyTemperature(r.probs, temperature), kind: "CALIBRATED_LOGIT" };
}

/** Options a request exposes: BELIEF/GATE/VERIFY are yes/no, SCORE uses `scale`, else candidates. */
export function optionsOf(req: DecisionRequestV1): string[] {
  switch (req.operation) {
    case "BELIEF": case "GATE": case "VERIFY": return ["true", "false"];
    case "SCORE": return (req.scale ?? []).map((_, i) => String(i));
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
