import { describe, expect, test } from "bun:test";
import {
  DecisionRouter, FixtureReadoutProvider, MOTIFS, actionFor, applyTemperature, buildManifest, buildRequest, candidateSchemaHash, candidateSetHash,
  checkBinding, coverageAtRisk, ece, evaluateQ22Gate, fitTemperature, macroF1, orderSensitivity, flipSensitivity, scopeFingerprint, validateProfile, wilsonUpper,
  DEFAULT_PROFILE, type CalibrationScope, type DecisionRequestV1, type Sample,
} from "../src/decision/index.ts";

const env = { abi_version: "1.0.0", schema_id: "allternit.kernel.DecisionRequestV1" };
const req: DecisionRequestV1 = buildRequest("ROUTE", {
  envelope: env, state_projection_ref: "state.x", instructions: "pick", decision_bank_id: "bank.a", question_id: "q.route",
  candidates: [{ candidate_id: "a" }, { candidate_id: "b" }],
});
const deployment = { backend_id: "backend.t", model_ref: "model.test", model_revision: "r1", tokenizer_id: "tok", quantization: "q8", runtime_backend: "rt", readout_point: null };
const scopeFor = (r: DecisionRequestV1): CalibrationScope => ({
  ...deployment, question_id: r.question_id!, candidate_schema_hash: candidateSchemaHash(r), candidate_set_hash: candidateSetHash(r), threshold_profile: DEFAULT_PROFILE.profile_id,
});

// Deterministic well-calibrated synthetic set: confidence c is right with prob c.
function synth(n: number, conf: number, wrongEvery: number): Sample[] {
  return Array.from({ length: n }, (_, i) => {
    const right = (i + 1) % wrongEvery !== 0;
    return { probs: right ? [conf, 1 - conf] : [1 - conf, conf], label: 0 } as Sample;
  }).map((s, i) => ((i + 1) % wrongEvery === 0 ? { probs: [conf, 1 - conf], label: 1 } : s));
}
const goodSet = () => synth(400, 0.97, 33); // ~3% errors at conf .97
const manifestFor = (ss: Sample[], scope = scopeFor(req)) => buildManifest({ manifest_id: "cal.1", primitive_id: "dec.choice", scope, heldOut: ss, autoMinConfidence: 0.95 });

describe("metrics", () => {
  test("perfect calibration has ECE 0; overconfidence is penalized", () => {
    expect(ece([{ probs: [1, 0], label: 0 }, { probs: [1, 0], label: 0 }])).toBe(0);
    expect(ece([{ probs: [0.99, 0.01], label: 1 }, { probs: [0.99, 0.01], label: 0 }])).toBeGreaterThan(0.4);
  });
  test("macro F1 and coverage-at-risk", () => {
    expect(macroF1([{ probs: [1, 0], label: 0 }, { probs: [0, 1], label: 1 }])).toBe(1);
    const ss: Sample[] = [{ probs: [0.99, 0.01], label: 0 }, { probs: [0.9, 0.1], label: 0 }, { probs: [0.6, 0.4], label: 1 }];
    expect(coverageAtRisk(ss, 0)).toBeCloseTo(2 / 3);
  });
  test("flip and order sensitivity count argmax changes", () => {
    const ss: Sample[] = [
      { probs: [0.9, 0.1], label: 0, probs_flipped: [0.2, 0.8], probs_reordered: [0.8, 0.2] },
      { probs: [0.9, 0.1], label: 0, probs_flipped: [0.7, 0.3], probs_reordered: [0.3, 0.7] },
    ];
    expect(flipSensitivity(ss)).toBe(0.5);
    expect(orderSensitivity(ss)).toBe(0.5);
  });
  test("wilson upper bound exceeds the point estimate", () => {
    expect(wilsonUpper(5, 100)).toBeGreaterThan(0.05);
    expect(wilsonUpper(0, 100)).toBeLessThan(0.03);
  });
  test("temperature fit softens an overconfident model", () => {
    const over: Sample[] = Array.from({ length: 200 }, (_, i) => ({ probs: [0.99, 0.01], label: i % 4 === 0 ? 1 : 0 }));
    const T = fitTemperature(over);
    expect(T).toBeGreaterThan(1);
    expect(applyTemperature([0.99, 0.01], T)[0]).toBeLessThan(0.99);
  });
});

describe("Q22 gate", () => {
  test("passes a well-calibrated, low-error, large-enough held-out set", () => {
    const m = manifestFor(goodSet());
    expect(evaluateQ22Gate(m).failures).toEqual([]);
    expect(m.gate.passed).toBe(true);
    expect(m.gate.agreement_with_other_model_used).toBe(false);
  });
  test("fails on ECE", () => {
    const m = manifestFor(synth(400, 0.99, 4)); // 25% errors at .99 conf
    expect(evaluateQ22Gate(m).passed).toBe(false);
    expect(m.gate.passed).toBe(false);
  });
  test("fails on too-small sample even with perfect numbers", () => {
    const m = manifestFor(synth(50, 0.97, 1000));
    expect(evaluateQ22Gate(m).failures.join()).toContain("held-out n");
  });
  test("fails when auto-act error > 5%", () => {
    const m = manifestFor(synth(400, 0.97, 10));
    expect(evaluateQ22Gate(m).passed).toBe(false);
  });
  test("agreement-with-another-model can never satisfy the gate", () => {
    const m = manifestFor(goodSet());
    (m.gate as any).agreement_with_other_model_used = true;
    expect(evaluateQ22Gate(m).passed).toBe(false);
  });
  test("strictBounds requires upper bounds inside limits", () => {
    const m = manifestFor(synth(400, 0.97, 22)); // ~4.5% observed, upper bound > 5%
    expect(evaluateQ22Gate(m).passed).toBe(true);
    expect(evaluateQ22Gate(m, { strictBounds: true }).passed).toBe(false);
  });
  test("floors cannot be lowered", () => {
    expect(evaluateQ22Gate(manifestFor(synth(50, 0.97, 1000)), { minHeldOutN: 1, minAutoActN: 1 }).passed).toBe(false);
  });
});

describe("manifest binding", () => {
  test("exact scope binds; any drift breaks it", () => {
    const m = manifestFor(goodSet());
    expect(checkBinding(m, scopeFor(req)).ok).toBe(true);
    for (const k of ["model_revision", "runtime_backend", "question_id", "candidate_schema_hash", "candidate_set_hash", "model_ref"] as const) {
      expect(checkBinding(m, { ...scopeFor(req), [k]: "sha256:" + "0".repeat(64) }).ok).toBe(false);
    }
  });
  test("tampered scope fails the fingerprint", () => {
    const m = manifestFor(goodSet());
    m.scope.model_revision = "r2";
    expect(checkBinding(m, m.scope).ok).toBe(false);
    expect(scopeFingerprint(m.scope)).not.toBe(m.scope_fingerprint);
  });
});

describe("router", () => {
  const provider = (p: number[]) => new FixtureReadoutProvider("backend.t", p, deployment as any);
  test("shadow default + no manifest: refuses S1 (UNCALIBRATED, abstained, never AUTO)", async () => {
    const r = await new DecisionRouter({ provider: provider([0.999, 0.001]), manifests: [] }).decide(req, "s", { reversible: true });
    expect(r.confidence_semantics).toBe("UNCALIBRATED");
    expect(r.calibration_level_served).toBe("NONE");
    expect(r.abstained).toBe(true);
    expect(r.threshold_action).not.toBe("AUTO");
    expect((r.extensions as any)["x-refused_uncalibrated"]).toBe(true);
  });
  test("failing manifest is refused", async () => {
    const bad = manifestFor(synth(400, 0.99, 4));
    const r = await new DecisionRouter({ provider: provider([0.999, 0.001]), manifests: [bad], mode: "live" }).decide(req, "s", { reversible: true });
    expect(r.threshold_action).not.toBe("AUTO");
    expect(r.confidence_semantics).toBe("UNCALIBRATED");
  });
  test("passing manifest in SHADOW mode is calibrated but AUTO is downgraded", async () => {
    const r = await new DecisionRouter({ provider: provider([0.999, 0.001]), manifests: [manifestFor(goodSet())] }).decide(req, "s", { reversible: true });
    expect(r.confidence_semantics).toBe("CALIBRATED");
    expect(r.threshold_action).toBe("REVIEW");
  });
  test("live + passing manifest + reversible => AUTO; not reversible => REVIEW", async () => {
    const cfg = { provider: provider([0.999, 0.001]), manifests: [manifestFor(goodSet())], mode: "live" as const };
    expect((await new DecisionRouter(cfg).decide(req, "s", { reversible: true })).threshold_action).toBe("AUTO");
    expect((await new DecisionRouter(cfg).decide(req, "s", {})).threshold_action).toBe("REVIEW");
  });
  test("live router refuses calibration for a different deployed readout head", async () => {
    const runtimeDeployment = { ...deployment, readout_point: "head-new" };
    const runtimeScope = { ...scopeFor(req), ...runtimeDeployment };
    const old = manifestFor(goodSet(), { ...runtimeScope, readout_point: "head-old" });
    expect(old.gate.passed).toBe(true);
    expect(checkBinding(old, runtimeScope).ok).toBe(false);
    const r = await new DecisionRouter({
      provider: new FixtureReadoutProvider("backend.t", [0.999, 0.001], runtimeDeployment),
      manifests: [old], mode: "live", primitiveId: "dec.choice",
    }).decide(req, "s", { reversible: true });
    expect(r.threshold_action).toBe("REVIEW");
    expect(r.confidence_semantics).toBe("UNCALIBRATED");
    expect(r.abstained).toBe(true);
    expect(r.calibration_id).toBeNull();
    expect((r.extensions!["x-reasons"] as string[]).join()).toContain("scope mismatch");
    const bound = manifestFor(goodSet(), runtimeScope);
    const live = await new DecisionRouter({
      provider: new FixtureReadoutProvider("backend.t", [0.999, 0.001], runtimeDeployment),
      manifests: [old, bound], mode: "live", primitiveId: "dec.choice",
    }).decide(req, "s", { reversible: true });
    expect(live.threshold_action).toBe("AUTO");
    expect(live.calibration_id).toBe(bound.manifest_id);
  });
  test("manifest bound to a different candidate set does not serve", async () => {
    const other = buildRequest("ROUTE", { envelope: env, state_projection_ref: "s", instructions: "x", decision_bank_id: "b", question_id: "q.route", candidates: [{ candidate_id: "a" }, { candidate_id: "c" }] });
    const r = await new DecisionRouter({ provider: provider([0.999, 0.001]), manifests: [manifestFor(goodSet())], mode: "live" }).decide(other, "s", { reversible: true });
    expect(r.confidence_semantics).toBe("UNCALIBRATED");
  });
  test("outside coverage region abstains", async () => {
    const m = manifestFor(goodSet());
    m.coverage_region = { ...m.coverage_region, max_candidates: 2 }; // request has 3 (incl. unknown)
    const r = await new DecisionRouter({ provider: provider([0.9, 0.05, 0.05]), manifests: [m], mode: "live" }).decide(req, "s", { reversible: true });
    expect(r.abstained).toBe(true);
  });
});

describe("threshold policy + motifs", () => {
  test("actions by confidence band", () => {
    expect([0.99, 0.8, 0.4, 0.1].map((c) => actionFor(DEFAULT_PROFILE, c))).toEqual(["AUTO", "REVIEW", "ESCALATE", "REJECT"]);
  });
  test("invalid profile rejected", () => {
    expect(validateProfile({ ...DEFAULT_PROFILE, review_min: 0.99 })).not.toEqual([]);
  });
  test("all ten motifs exist and build valid operations; ROUTE adds an unknown candidate", () => {
    expect(Object.keys(MOTIFS)).toHaveLength(10);
    expect(req.candidates!.some((c) => c.is_unknown)).toBe(true);
    expect(buildRequest("REFLEX", { envelope: env, state_projection_ref: "s", instructions: "i", decision_bank_id: "b", question_id: "q", candidates: [{ candidate_id: "a" }, { candidate_id: "b" }] }).latency_class).toBe("REALTIME");
    expect(buildRequest("GATE", { envelope: env, state_projection_ref: "s", instructions: "i", decision_bank_id: "b", question_id: "q" }).operation).toBe("GATE");
    expect(() => buildRequest("JUDGE", { envelope: env, state_projection_ref: "s", instructions: "i", decision_bank_id: "b", question_id: "q" })).toThrow();
  });
});

describe("HTTP /v1/decision", () => {
  test("backend laya_bundled is decided by Laya, recorded under its own backend_id", async () => {
    const { createHandler } = await import("../src/server.ts");
    const { SystemOne } = await import("../src/engine.ts");
    const runtime = { name: "fake", model: "m", async complete(): Promise<any> { throw new Error("local runtime must not be called"); } };
    const urls: string[] = [];
    const fetchImpl = async (u: string, i?: RequestInit) => {
      urls.push(u);
      const q = JSON.parse(String(i!.body)).questions.q;
      const ids = Object.keys(q.criteria ?? {});
      return new Response(JSON.stringify({ answers: { q: { type: q.type, choice: ids[0], probabilities: Object.fromEntries(ids.map((k, j) => [k, j === 0 ? 0.9 : 0.1 / (ids.length - 1)])), noul: 0.9 } }, usage: { input_tokens: 3, output_tokens: 0 } }));
    };
    const engine = new SystemOne({ runtimeUrl: "x", runtimeModel: "m", concurrency: 1, samples: 2, debias: false, layaUrl: "http://laya", layaModel: "typed-decisions", logEnabled: false }, { runtime, fetchImpl });
    const res = await createHandler({ engine })(new Request("http://x/v1/decision", { method: "POST", body: JSON.stringify({ request: req, state: "s", reversible: true, backend: "laya_bundled" }) }));
    expect(res.status).toBe(200);
    const body: any = await res.json();
    expect(urls).toEqual(["http://laya/v1/systemone"]);
    expect(JSON.stringify(body)).toContain("backend.laya");
    expect(body.threshold_action).not.toBe("AUTO");
  });
  test("backend jev_api without a key is refused, never silently local", async () => {
    const { createHandler } = await import("../src/server.ts");
    const { SystemOne } = await import("../src/engine.ts");
    const runtime = { name: "fake", model: "m", async complete(): Promise<any> { throw new Error("must not run"); } };
    const engine = new SystemOne({ runtimeUrl: "x", runtimeModel: "m", concurrency: 1, samples: 2, debias: false, layaUrl: "http://laya", layaModel: "typed-decisions", logEnabled: false }, { runtime });
    const res = await createHandler({ engine })(new Request("http://x/v1/decision", { method: "POST", body: JSON.stringify({ request: req, state: "s", backend: "jev_api" }) }));
    expect(res.status).toBe(401);
  });
  test("default server refuses uncalibrated S1 through the ABI route", async () => {
    const { createHandler } = await import("../src/server.ts");
    const { SystemOne } = await import("../src/engine.ts");
    const runtime = {
      name: "fake", model: "m",
      async complete() { return { text: "a", top: [{ token: "a", logprob: Math.log(0.99) }, { token: "b", logprob: Math.log(0.01) }], usage: { input: 1, output: 1 } }; },
    };
    const engine = new SystemOne({ runtimeUrl: "x", runtimeModel: "m", concurrency: 1, samples: 2, debias: false, layaUrl: "http://laya", layaModel: "typed-decisions", logEnabled: false }, { runtime });
    const res = await createHandler({ engine })(new Request("http://x/v1/decision", { method: "POST", body: JSON.stringify({ request: req, state: "s", reversible: true }) }));
    expect(res.status).toBe(200);
    const body: any = await res.json();
    expect(body.confidence_semantics).toBe("UNCALIBRATED");
    expect(body.threshold_action).not.toBe("AUTO");
    expect(body.envelope.schema_id).toBe("allternit.kernel.DecisionResultV1");
  });
});

describe("remaining operations via the local logit provider", () => {
  const { LocalLogitReadoutProvider } = require("../src/decision/index.ts");
  const { SystemOne } = require("../src/engine.ts");
  const P = (dist: Record<string, number>) => ({
    name: "fake", model: "m",
    calls: [] as string[],
    async complete(r: any) {
      this.calls.push(r.messages.map((m: any) => m.content).join("\n"));
      return { text: "", top: Object.entries(dist).map(([token, p]) => ({ token, logprob: Math.log(p) })), usage: { input: 1, output: 1 } };
    },
  });
  const mk = (rt: any) => new LocalLogitReadoutProvider(new SystemOne({ runtimeUrl: "x", runtimeModel: "m", concurrency: 1, samples: 2, debias: false, logEnabled: false }, { runtime: rt }), deployment);
  const base = { envelope: env, state_projection_ref: "s", instructions: "i", decision_bank_id: "b", question_id: "q" };

  test("RANK orders candidates by probability", async () => {
    const r = await new DecisionRouter({ provider: mk(P({ A: 0.2, B: 0.7, C: 0.1 })), manifests: [] }).decide({ ...base, operation: "RANK", candidates: [{ candidate_id: "x" }, { candidate_id: "y" }, { candidate_id: "z" }] }, "s");
    expect(r.answer).toEqual(["y", "x", "z"]);
    expect(r.abstained).toBe(true);
  });
  test("SUBSET asks one yes/no per candidate and keeps P>=0.5", async () => {
    const rt = P({ Yes: 0.9, No: 0.1 });
    const prov = mk(rt);
    const out = await prov.readout({ ...base, operation: "SUBSET", candidates: [{ candidate_id: "x" }, { candidate_id: "y" }] }, "s");
    expect(out.shape).toBe("independent");
    expect(rt.calls.length).toBeGreaterThanOrEqual(2);
    expect(out.probs).toHaveLength(2);
    expect(out.probs.every((p: number) => p >= 0 && p <= 1)).toBe(true);
  });
  test("ESTIMATE returns an expected level", async () => {
    const r = await new DecisionRouter({ provider: mk(P({ A: 0.1, B: 0.1, C: 0.8 })), manifests: [] }).decide({ ...base, operation: "ESTIMATE", scale: ["low", "mid", "high"] }, "s");
    expect((r.answer as any).level).toBeDefined();
    const fx = await new DecisionRouter({ provider: new FixtureReadoutProvider("b", [0.1, 0.1, 0.8], deployment as any), manifests: [] }).decide({ ...base, operation: "ESTIMATE", scale: ["low", "mid", "high"] }, "s");
    expect((fx.answer as any).level).toBe("high");
  });
  test("PAIR_SCORE runs both orders and is DEBIASED", async () => {
    const rt = P({ A: 0.8, B: 0.2 }); // always prefers first-listed: position bias cancels to 50/50
    const out = await mk(rt).readout({ ...base, operation: "PAIR_SCORE", candidates: [{ candidate_id: "x" }, { candidate_id: "y" }] }, "s");
    expect(rt.calls).toHaveLength(2);
    expect(out.kind).toBe("DEBIASED_LOGIT");
    expect(out.probs[0]).toBeCloseTo(0.5, 1);
  });
  test("PAIR_SCORE rejects != 2 candidates", async () => {
    await expect(mk(P({ A: 1 })).readout({ ...base, operation: "PAIR_SCORE", candidates: [{ candidate_id: "x" }] }, "s")).rejects.toThrow();
  });
  test("independent readouts calibrate per option, not by softmax", () => {
    const { calibrateProbs } = require("../src/decision/index.ts");
    const c = calibrateProbs([0.9, 0.1], 2, "independent");
    expect(c[0]).toBeLessThan(0.9); expect(c[0]).toBeGreaterThan(0.5);
    expect(c[0] + c[1]).toBeCloseTo(1, 5); // symmetric here, but not forced to sum by softmax
  });
});

describe("manifest source shared with the ModelPool", () => {
  test("ALLTERNIT_S1_MANIFESTS unset or unreadable -> no manifests (fail closed)", async () => {
    const { loadManifests } = await import("../src/server");
    expect(loadManifests(undefined)).toEqual([]);
    expect(loadManifests("/nonexistent/manifests.json")).toEqual([]);
  });
  test("reads a JSON array of manifests", async () => {
    const { loadManifests } = await import("../src/server");
    const f = `${require("node:os").tmpdir()}/s1-manifests-${Date.now()}.json`;
    require("node:fs").writeFileSync(f, JSON.stringify([{ manifest_id: "m1" }]));
    expect(loadManifests(f)).toEqual([{ manifest_id: "m1" }]);
  });
});
