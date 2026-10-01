// Readout provider backed by the local typed-decision engine (token logprobs over
// constrained labels). This is the canonical S1 path: everything else is an adapter.
import type { SystemOne } from "../engine.ts";
import type { Question } from "../types.ts";
import type { DecisionRequestV1 } from "./contract.ts";
import { optionsOf, type DecisionReadoutProvider, type Readout } from "./readout.ts";

export interface LocalDeployment {
  model_ref: string; model_revision: string; tokenizer_id: string; quantization: string; runtime_backend: string;
}

/**
 * Laya's choice head shares a ~192-token budget across options and degrades
 * past ~16-20 (their recommendation: coarse-to-fine). Larger menus are decided
 * in two passes: a choice over <=16 consecutive groups, then within the winner;
 * p(option) = p(group) * p(option | group), other groups' mass spread evenly.
 */
export const LAYA_MAX_DIRECT_OPTIONS = 16;

export class LocalLogitReadoutProvider implements DecisionReadoutProvider {
  readonly backend_id: string;
  /** `model` picks the engine backend: "local" (logprobs), "laya:<checkpoint>", "typesafe:<model>". */
  constructor(private engine: SystemOne, private dep: LocalDeployment, backendId = "backend.local_logit", private model = "local") {
    this.backend_id = backendId;
  }

  private async ask(state: string, q: Question) {
    const res = await this.engine.evaluate({ model: this.model, state, questions: { q } });
    return { a: res.answers.q, res };
  }
  private criteriaOf(req: DecisionRequestV1) {
    const c = req.extensions?.["x-criteria"] as { true?: string; false?: string } | undefined;
    return c;
  }

  async readout(req: DecisionRequestV1, state: string): Promise<Readout> {
    const options = optionsOf(req);
    const cands = req.candidates ?? [];
    const usage = { input_tokens: 0, output_tokens: 0 };
    const t0 = performance.now();
    let methods: string[] = [];
    const track = (res: { usage: { input_tokens: number; output_tokens: number }; x_allternit?: { methods: Record<string, string> } }) => {
      usage.input_tokens += res.usage.input_tokens; usage.output_tokens += res.usage.output_tokens;
      methods.push(res.x_allternit?.methods.q ?? "sampled");
    };
    const choiceQ = (list: typeof cands): Question => ({ type: "choice", instructions: req.instructions, criteria: Object.fromEntries(list.map((c) => [c.candidate_id, c.label ?? c.candidate_id])) });
    const probsOf = (a: any, ids: string[]) => ids.map((o) => (a.probabilities as Record<string, number>)[o] ?? 0);
    let probs: number[];
    let shape: Readout["shape"] = "categorical";
    switch (req.operation) {
      case "BELIEF": case "GATE": case "VERIFY": {
        const { a, res } = await this.ask(state, { type: "noul", instructions: req.instructions, criteria: this.criteriaOf(req) });
        track(res); probs = [(a as any).noul, 1 - (a as any).noul]; break;
      }
      case "SCORE": case "ESTIMATE": {
        const { a, res } = await this.ask(state, { type: "score", instructions: req.instructions, criteria: req.scale ?? [] });
        track(res); probs = probsOf(a, options); break;
      }
      case "CHOICE": case "RANK": {
        // RANK: distribution over "which candidate is best", ordered by probability.
        if (this.model.startsWith("laya:") && cands.length > LAYA_MAX_DIRECT_OPTIONS) {
          const n = Math.ceil(cands.length / LAYA_MAX_DIRECT_OPTIONS);
          const size = Math.ceil(cands.length / n);
          const groups = Array.from({ length: n }, (_, g) => cands.slice(g * size, (g + 1) * size)).filter((g) => g.length);
          const gq: Question = { type: "choice", instructions: req.instructions,
            criteria: Object.fromEntries(groups.map((g, i) => [`group_${i}`, g.map((c) => c.label ?? c.candidate_id).join(", ")])) };
          const coarse = await this.ask(state, gq); track(coarse.res);
          const pg = groups.map((_, i) => ((coarse.a as any).probabilities as Record<string, number>)[`group_${i}`] ?? 0);
          const win = pg.indexOf(Math.max(...pg));
          const fine = await this.ask(state, choiceQ(groups[win])); track(fine.res);
          const pIn = Object.fromEntries(probsOf(fine.a, groups[win].map((c) => c.candidate_id)).map((p, j) => [groups[win][j].candidate_id, p]));
          const byId: Record<string, number> = {};
          groups.forEach((g, i) => g.forEach((c) => { byId[c.candidate_id] = i === win ? pg[i] * (pIn[c.candidate_id] ?? 0) : pg[i] / g.length; }));
          const z = Object.values(byId).reduce((x, y) => x + y, 0) || 1;
          probs = options.map((o) => (byId[o] ?? 0) / z);
          break;
        }
        const { a, res } = await this.ask(state, choiceQ(cands));
        track(res); probs = probsOf(a, options); break;
      }
      case "SUBSET": {
        // One independent yes/no per candidate: P(include).
        shape = "independent";
        probs = [];
        for (const c of cands) {
          const { a, res } = await this.ask(state, { type: "noul", instructions: `${req.instructions}\n\nCandidate: ${c.label ?? c.candidate_id}. Should this candidate be included?` });
          track(res); probs.push((a as any).noul);
        }
        break;
      }
      case "PAIR_SCORE": {
        // Exactly two candidates; average forward and reversed order to cancel position bias.
        if (cands.length !== 2) throw new Error("PAIR_SCORE needs exactly 2 candidates");
        const f = await this.ask(state, choiceQ(cands)); track(f.res);
        const r = await this.ask(state, choiceQ([cands[1], cands[0]])); track(r.res);
        const pf = probsOf(f.a, options), pr = probsOf(r.a, options);
        const avg = [(pf[0] + pr[0]) / 2, (pf[1] + pr[1]) / 2];
        const z = avg[0] + avg[1] || 1;
        probs = [avg[0] / z, avg[1] / z]; break;
      }
      default: throw new Error(`operation ${req.operation} is not served by the logit provider`);
    }
    const allLogprobs = methods.every((m) => m === "logprobs");
    return {
      options, probs, shape, kind: req.operation === "PAIR_SCORE" ? "DEBIASED_LOGIT" : "RAW_LOGIT",
      method: allLogprobs ? "logprobs" : methods.includes("remote") ? "remote" : "sampled",
      latency_ms: Math.round(performance.now() - t0), deployment: { backend_id: this.backend_id, ...this.dep }, usage,
    };
  }
}
