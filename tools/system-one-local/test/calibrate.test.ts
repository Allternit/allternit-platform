// SYNTHETIC FIXTURES LIVE ONLY IN THIS FILE. Production code never generates data.
import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, existsSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  DEFAULT_PROFILE, DecisionRouter, FixtureReadoutProvider, ShadowLedger, buildRequest, calibrate, calibrateAndWrite, candidateSchemaHash, candidateSetHash,
  harvest, readDataset, writeDataset, type CalibrationScope, type DatasetRow, type DecisionRequestV1,
} from "../src/decision/index.ts";
import { loadManifests } from "../src/server.ts";

const req: DecisionRequestV1 = buildRequest("ROUTE", {
  envelope: { abi_version: "1.0.0", schema_id: "allternit.kernel.DecisionRequestV1" }, state_projection_ref: "s", instructions: "classify the error",
  decision_bank_id: "bank.a", question_id: "q.err", candidates: [{ candidate_id: "a" }, { candidate_id: "b" }],
});
const deployment = { backend_id: "backend.t", model_ref: "model.test", model_revision: "r1", tokenizer_id: "tok", quantization: "q8", runtime_backend: "rt", readout_point: null };
const scope: CalibrationScope = { ...deployment, question_id: "q.err", candidate_schema_hash: candidateSchemaHash(req), candidate_set_hash: candidateSetHash(req), threshold_profile: DEFAULT_PROFILE.profile_id };
const tmp = () => mkdtempSync(join(tmpdir(), "s1cal-"));

// Seeded RNG so fixtures are reproducible.
function rng(seed: number) { let x = seed >>> 0; return () => ((x = (Math.imul(x, 1664525) + 1013904223) >>> 0) / 2 ** 32); }

/** groups: [share, stated confidence, true accuracy]. Labels drawn so the stated confidence is right at `acc`. */
function synthRows(n: number, groups: [number, number, number][], seed = 7): DatasetRow[] {
  const r = rng(seed);
  return Array.from({ length: n }, (_, i) => {
    let u = r(), g = groups[groups.length - 1];
    for (const c of groups) { if (u < c[0]) { g = c; break; } u -= c[0]; }
    const [, conf, acc] = g;
    const predictedA = r() < 0.5;
    const rest = 1 - conf;
    const probs = predictedA ? [conf, rest * 0.9, rest * 0.1] : [rest * 0.9, conf, rest * 0.1];
    const correct = r() < acc;
    const label_index = correct ? (predictedA ? 0 : 1) : predictedA ? 1 : 0;
    return {
      decision_id: `d${i}`, ts: new Date(Date.UTC(2026, 8, 1) + i * 60_000).toISOString(), primitive_id: "dec.choice", operation: "ROUTE",
      question: { question_id: "q.err", instructions: "classify the error" }, candidates: req.candidates!.map((c) => ({ candidate_id: c.candidate_id, label: null })),
      options: req.candidates!.map((c) => c.candidate_id), readout: { probs, method: "fixture" }, label: req.candidates![label_index].candidate_id, label_index, scope,
      provenance: { decision_log: "x", outcome_source: "synthetic-test", outcome_ts: "", state_sha256: "", subject_ref: null },
    };
  });
}

describe("calibrate CLI core", () => {
  test("well-calibrated set passes the gate and appends a usable manifest atomically", () => {
    const dir = tmp(), mp = join(dir, "manifests.json");
    writeFileSync(mp, "[]");
    const rows = synthRows(1400, [[0.7, 0.98, 0.98], [0.3, 0.65, 0.65]]);
    const rep = calibrateAndWrite(rows, { primitive: "dec.choice", model: "model.test", manifestsPath: mp, reportPath: join(dir, "rep.json") });
    expect(rep.scopes[0].failures).toEqual([]);
    expect(rep.passed).toBe(true);
    const m = rep.scopes[0].manifest!;
    expect(m.held_out.n).toBeGreaterThanOrEqual(300);
    expect(m.metrics.ece!).toBeLessThanOrEqual(0.05);
    expect(m.held_out.auto_act_error_rate).toBeLessThanOrEqual(0.05);
    expect(m.gate.agreement_with_other_model_used).toBe(false);
    // held-out is strictly later than the fit window (time-based split)
    expect(rep.scopes[0].held_out_window![0] > rep.scopes[0].fit_window![1]).toBe(true);
    // second run appends rather than clobbers
    calibrateAndWrite(rows, { primitive: "dec.choice", model: "model.test", manifestsPath: mp, reportPath: join(dir, "rep.json"), now: new Date(Date.now() + 5000) });
    expect(JSON.parse(readFileSync(mp, "utf8")).length).toBe(2);
  });

  test("the written manifest actually turns the router calibrated (gate green, no hand work)", async () => {
    const dir = tmp(), mp = join(dir, "manifests.json");
    calibrateAndWrite(synthRows(1400, [[0.7, 0.98, 0.98], [0.3, 0.65, 0.65]]), { primitive: "dec.choice", model: "model.test", manifestsPath: mp, reportPath: join(dir, "r.json") });
    const router = new DecisionRouter({ provider: new FixtureReadoutProvider("backend.t", [0.99, 0.009, 0.001], deployment as any), manifests: loadManifests(mp), primitiveId: "dec.choice", mode: "live" });
    const out = await router.decide({ ...req, decision_bank_id: "dec.choice" }, "s", { reversible: true });
    expect((out.extensions as any)["x-reasons"]).toEqual([]);
    expect(out.confidence_semantics).toBe("CALIBRATED");
    expect(out.calibration_id).toMatch(/^cal\.dec\.choice\./);
  });

  test("miscalibrated / inaccurate set fails, writes NO manifest, and reports why", () => {
    const dir = tmp(), mp = join(dir, "manifests.json"), rp = join(dir, "rep.json");
    const rep = calibrateAndWrite(synthRows(1400, [[1, 0.99, 0.75]]), { primitive: "dec.choice", model: "model.test", manifestsPath: mp, reportPath: rp });
    expect(rep.passed).toBe(false);
    expect(rep.manifests_written).toEqual([]);
    expect(existsSync(mp)).toBe(false);
    expect(rep.scopes[0].failures.length).toBeGreaterThan(0);
    expect(JSON.parse(readFileSync(rp, "utf8")).scopes[0].gate_passed).toBe(false);
  });

  test("too little data fails with a data-needed message, no manifest", () => {
    const rep = calibrate(synthRows(100, [[1, 0.98, 0.98]]), { primitive: "dec.choice", model: "model.test" });
    expect(rep.passed).toBe(false);
    expect(rep.scopes[0].failures[0]).toContain("insufficient data");
    expect(rep.scopes[0].manifest).toBeNull();
  });

  test("primitive/model filters; refuses to clobber a non-array manifests file", () => {
    expect(calibrate(synthRows(50, [[1, 0.9, 0.9]]), { primitive: "other", model: "model.test" }).passed).toBe(false);
    const dir = tmp(), mp = join(dir, "m.json");
    writeFileSync(mp, "{}");
    expect(() => calibrateAndWrite(synthRows(1400, [[0.7, 0.98, 0.98], [0.3, 0.65, 0.65]]), { primitive: "dec.choice", model: "model.test", manifestsPath: mp, reportPath: join(dir, "r.json") })).toThrow();
  });
});

describe("shadow ledger + harvester", () => {
  const flush = () => new Promise((r) => setTimeout(r, 5));

  test("router logs shadow decisions (state hashed) and harvest joins outcomes by id and subject_ref", async () => {
    const dir = tmp();
    const ledger = new ShadowLedger(dir);
    const router = new DecisionRouter({ provider: new FixtureReadoutProvider("backend.t", [0.8, 0.15, 0.05], deployment as any), manifests: [], primitiveId: "dec.choice", ledger });
    const r1 = await router.decide(req, "SECRET STATE 1", {});
    const r2 = await router.decide({ ...req, extensions: { "x-subject_ref": "run-42" } }, "SECRET STATE 2", {});
    const r3 = await router.decide(req, "no outcome for this one", {});
    await flush();
    const id1 = r1.extensions!["x-decision_id"] as string;
    expect(id1).toBeTruthy();
    expect(r2.extensions!["x-decision_id"]).not.toBe(id1);
    ledger.recordOutcome({ decision_id: id1, truth: "b", source: "verifier:tests" });
    ledger.recordOutcome({ subject_ref: "run-42", question_id: "q.err", truth: "a", source: "parser:tsc" });
    ledger.recordOutcome({ decision_id: "ghost", truth: "a", source: "verifier:tests" });
    void r3;
    const { rows, stats } = harvest(dir, { primitive: "dec.choice" });
    expect(stats).toMatchObject({ decisions: 3, joined: 2, no_outcome: 1, unmatched_outcomes: 1 });
    const byLabel = Object.fromEntries(rows.map((r) => [r.decision_id, r]));
    expect(byLabel[id1].label).toBe("b");
    expect(byLabel[id1].label_index).toBe(1);
    expect(byLabel[id1].readout.probs).toEqual([0.8, 0.15, 0.05]);
    expect(byLabel[id1].provenance.outcome_source).toBe("verifier:tests");
    expect(byLabel[id1].provenance.decision_log).toMatch(/^decisions\//);
    expect(byLabel[r2.extensions!["x-decision_id"] as string].label).toBe("a");
    expect(byLabel[r2.extensions!["x-decision_id"] as string].provenance.subject_ref).toBe("run-42");
    // privacy: raw state never hits disk
    const all = JSON.stringify(rows);
    expect(all).not.toContain("SECRET STATE");
    // dataset round trip
    const out = join(dir, "ds.jsonl");
    writeDataset(rows, out);
    expect(readDataset(out).length).toBe(2);
  });

  test("a subject_ref outcome labels the latest decision of every primitive on that subject (WP-S1U-3)", async () => {
    const dir = tmp();
    const ledger = new ShadowLedger(dir);
    const base = { operation: "GATE", instructions: "i", subject_ref: "cc-tool:t1", candidates: [], options: ["true", "false"], shape: "categorical" as const, probs: [0.5, 0.5], readout_method: "fixture", scope: {} as any, mode: "shadow", state: "s" };
    const ts = (n: number) => new Date(Date.UTC(2026, 9, 1, 0, 0, n)).toISOString();
    const guardOld = ledger.logDecision({ ...base, primitive_id: "permission.cli_guard", question_id: "may_proceed", ts: ts(1) });
    const guard = ledger.logDecision({ ...base, primitive_id: "permission.cli_guard", question_id: "may_proceed", ts: ts(2) });
    const judge = ledger.logDecision({ ...base, primitive_id: "permission.commrails_judge", question_id: "may_proceed", ts: ts(2) });
    const first = ledger.logDecision({ ...base, primitive_id: "judge.first_pass.tool", question_id: "tool_safe", ts: ts(2) });
    await flush();
    ledger.recordOutcome({ subject_ref: "cc-tool:t1", truth: "false", source: "cli_hook.permission_denied", ts: ts(3) });
    const { rows } = harvest(dir);
    const labelled = new Set(rows.map((r) => r.decision_id));
    expect(labelled).toEqual(new Set([guard, judge, first]));
    expect(labelled.has(guardOld)).toBe(false);
    expect(rows.every((r) => r.label === "false")).toBe(true);
  });

  test("latest outcome wins; truth outside options and independent-shape rows are skipped, not guessed", async () => {
    const dir = tmp();
    const l = new ShadowLedger(dir);
    const base = { primitive_id: "p", operation: "CHOICE", question_id: "q", instructions: "i", subject_ref: null, state: "s", candidates: [] as { candidate_id: string; label: string | null }[], options: ["x", "y"], readout_method: "fixture", scope, mode: "shadow" };
    const a = l.logDecision({ ...base, shape: "categorical", probs: [0.6, 0.4], ts: "2026-09-01T00:00:00.000Z" });
    const b = l.logDecision({ ...base, shape: "categorical", probs: [0.6, 0.4], ts: "2026-09-01T00:00:01.000Z" });
    const c = l.logDecision({ ...base, shape: "independent", probs: [0.6, 0.4], ts: "2026-09-01T00:00:02.000Z" });
    await flush();
    l.recordOutcome({ decision_id: a, truth: "x", source: "s1", ts: "2026-09-01T01:00:00.000Z" });
    l.recordOutcome({ decision_id: a, truth: "y", source: "s2", ts: "2026-09-01T02:00:00.000Z" });
    l.recordOutcome({ decision_id: b, truth: "zzz", source: "s1" });
    l.recordOutcome({ decision_id: c, truth: "x", source: "s1" });
    const { rows, stats } = harvest(dir);
    expect(rows.map((r) => r.label)).toEqual(["y"]);
    expect(stats.truth_not_in_options).toBe(1);
    expect(stats.skipped_independent).toBe(1);
  });

  test("an outcome without a source or join key is rejected (no anonymous labels)", () => {
    const l = new ShadowLedger(tmp());
    expect(() => l.recordOutcome({ decision_id: "x", truth: "a", source: "" })).toThrow();
    expect(() => l.recordOutcome({ truth: "a", source: "v" })).toThrow();
  });
});
