// WP-L1: ledger export, the Q26 gate, canary exposure budget + CUSUM rollback, Laya revision identity.
// SYNTHETIC FIXTURES LIVE ONLY IN THIS FILE. Production code never generates data.
import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  CanaryController, DEFAULT_PROFILE, DecisionRouter, FixtureReadoutProvider, ShadowLedger, buildExport, buildRequest, candidateSchemaHash, candidateSetHash,
  clopperPearsonUpper, chooseTau, consequenceFor, evaluateQ26Manifest, layaQuestion, runQ26, unitHash, writeExport,
  type CalibrationScope, type DatasetRow, type DecisionRequestV1,
} from "../src/decision/index.ts";
import { layaRevision, LAYA_PINNED_REVISION } from "../src/server.ts";

const tmp = () => mkdtempSync(join(tmpdir(), "s1q26-"));
function rng(seed: number) { let x = seed >>> 0; return () => ((x = (Math.imul(x, 1664525) + 1013904223) >>> 0) / 2 ** 32); }

const req = (bank: string): DecisionRequestV1 => buildRequest("RANK", {
  envelope: { abi_version: "1.0.0", schema_id: "allternit.kernel.DecisionRequestV1" }, state_projection_ref: "s", instructions: "retry or stop",
  decision_bank_id: bank, question_id: "q.retry", candidates: [{ candidate_id: "retry" }, { candidate_id: "stop" }],
});
const deployment = { backend_id: "backend.t", model_ref: "model.test", model_revision: "ft-1", tokenizer_id: "tok", quantization: "none", runtime_backend: "rt", readout_point: null };
const scopeFor = (bank: string): CalibrationScope => ({ ...deployment, question_id: "q.retry", candidate_schema_hash: candidateSchemaHash(req(bank)), candidate_set_hash: candidateSetHash(req(bank)), threshold_profile: DEFAULT_PROFILE.profile_id });

/**
 * n rows; `share` of them are high-confidence (conf `hi`, true accuracy `hiAcc`), the rest at 0.6 / 0.6.
 * The incumbent is right with probability `incAcc` (or absent when incAcc is null).
 */
function rows(bank: string, n: number, o: { share?: number; hi?: number; hiAcc?: number; incAcc?: number | null; seed?: number; t0?: number } = {}): DatasetRow[] {
  const r = rng(o.seed ?? 11);
  const { share = 0.6, hi = 0.985, hiAcc = 0.99, incAcc = 0.95 } = o;
  return Array.from({ length: n }, (_, i) => {
    const isHi = r() < share;
    const c = isHi ? hi : 0.6, acc = isHi ? hiAcc : 0.6;
    const pred = r() < 0.5 ? 0 : 1;
    const label = r() < acc ? pred : 1 - pred;
    const probs = pred === 0 ? [c, 1 - c] : [1 - c, c];
    const options = ["retry", "stop"];
    const incumbent = incAcc === null ? undefined : r() < incAcc ? options[label] : options[1 - label];
    return {
      decision_id: `${bank}-${o.t0 ?? 0}-${i}`, ts: new Date(Date.UTC(2026, 9, 1) + ((o.t0 ?? 0) + i) * 1000).toISOString(), primitive_id: bank, operation: "CHOICE",
      question: { question_id: "q.retry", instructions: "retry or stop" }, candidates: options.map((x) => ({ candidate_id: x, label: null })), options,
      readout: { probs, method: "fixture" }, label: options[label], label_index: label, scope: scopeFor(bank),
      ...(incumbent ? { incumbent } : {}),
      provenance: { decision_log: "x", outcome_source: "verifier:test", outcome_ts: "x", state_sha256: "x", subject_ref: null },
    } as DatasetRow;
  });
}
const split = (bank: string, o: Parameters<typeof rows>[2] = {}) => ({ tune: rows(bank, 350, { ...o, seed: 5, t0: 0 }), cert: rows(bank, 600, { ...o, seed: 9, t0: 10_000 }) });

describe("exact bounds", () => {
  test("Clopper–Pearson one-sided upper bound matches closed forms", () => {
    // k = 0: upper = 1 - δ^(1/n)
    expect(clopperPearsonUpper(0, 59)).toBeCloseTo(1 - 0.05 ** (1 / 59), 5);
    expect(clopperPearsonUpper(0, 60)).toBeLessThan(0.05);
    expect(clopperPearsonUpper(5, 100)).toBeGreaterThan(0.05);
    expect(clopperPearsonUpper(5, 100)).toBeCloseTo(0.1023, 3);
    expect(clopperPearsonUpper(3, 0)).toBe(1);
  });
  test("τ walks down only while the bound holds (fixed-sequence LTT)", () => {
    const ss = rows("bank.x", 400).map((r) => ({ probs: r.readout.probs, label: r.label_index }));
    const tau = chooseTau(ss, 0.05);
    expect(tau).not.toBeNull();
    expect(tau!).toBeGreaterThan(0.6); // the 0.6-confidence rows are 40% wrong and must stay out
  });
});

describe("Q26 gate", () => {
  test("a good bank passes, gets a Q26 manifest bound to its revision, and is eligible", () => {
    const { tune, cert } = split("bank.retry");
    const rep = runQ26(tune, cert);
    expect(rep.banks["bank.retry"].eligible).toBe(true);
    const b = rep.bindings[0];
    expect(b.passed).toBe(true);
    expect(b.cert!.error_upper).toBeLessThanOrEqual(0.05);
    expect(b.cert!.coverage).toBeGreaterThanOrEqual(0.3);
    expect(b.manifest!.extensions!["x-gate"]).toBe("Q26");
    expect(b.manifest!.scope.model_revision).toBe("ft-1");
    expect(evaluateQ26Manifest(b.manifest!).passed).toBe(true);
    expect(rep.temperatures[0]).toMatchObject({ bank: "bank.retry", type: "CHOICE", k: 2 });
  });
  test("sample minimums: 299 tuning or 499 certification rows fail", () => {
    const rep = runQ26(rows("bank.retry", 299), rows("bank.retry", 600, { seed: 3, t0: 9000 }));
    expect(rep.banks["bank.retry"].eligible).toBe(false);
    expect(rep.bindings[0].failures.join()).toContain("tuning split n=299 < 300");
    const rep2 = runQ26(rows("bank.retry", 350), rows("bank.retry", 499, { seed: 3, t0: 9000 }));
    expect(rep2.bindings[0].failures.join()).toContain("certification split n=499 < 500");
  });
  test("permission/money/client banks never auto-act, whatever the data says", () => {
    for (const bank of ["bank.permission.v0", "bank.vendor_consequential.v0", "bank.client_email"]) expect(consequenceFor(bank)).toBe("never");
    const { tune, cert } = split("bank.permission.v0");
    const rep = runQ26(tune, cert, { policy: { "bank.permission.v0": { consequence: "default" } } });
    expect(rep.banks["bank.permission.v0"].eligible).toBe(false);
  });
  test("ε = 10% for cheap-retry banks only", () => {
    const o = { hiAcc: 0.95, share: 0.7, incAcc: 0.9 } as const;
    const { tune, cert } = split("bank.retry", o);
    expect(runQ26(tune, cert).banks["bank.retry"].eligible).toBe(false);
    const rep = runQ26(tune, cert, { policy: { "bank.retry": { consequence: "cheap_retry" } } });
    expect(rep.banks["bank.retry"].eligible).toBe(true);
    expect(rep.banks["bank.retry"].epsilon).toBe(0.1);
  });
  test("non-inferiority: missing incumbent answers or a better incumbent fail", () => {
    const missing = split("bank.retry", { incAcc: null });
    expect(runQ26(missing.tune, missing.cert).bindings[0].failures.join()).toContain("incumbent answers missing");
    expect(runQ26(missing.tune, missing.cert, { policy: { "bank.retry": { incumbent: "none" } } }).banks["bank.retry"].eligible).toBe(true);
    const better = split("bank.retry", { hiAcc: 0.96, incAcc: 1.0 });
    const rep = runQ26(better.tune, better.cert, { policy: { "bank.retry": { consequence: "cheap_retry" } } });
    expect(rep.bindings[0].noninferiority!.checked).toBe(true);
    expect(rep.bindings[0].failures.join()).toContain("inferior to the incumbent");
  });
  test("coverage below 30% fails", () => {
    const { tune, cert } = split("bank.retry", { share: 0.2, hiAcc: 1.0 });
    expect(runQ26(tune, cert).bindings[0].failures.join()).toContain("coverage");
  });
  test("overlapping splits are refused; a tampered manifest is rejected by the router check", () => {
    const { tune, cert } = split("bank.retry");
    expect(() => runQ26(tune, [...cert, tune[0]])).toThrow(/overlap/);
    const m = runQ26(tune, cert).bindings[0].manifest!;
    const bad = JSON.parse(JSON.stringify(m));
    bad.extensions["x-q26"].error_upper = 0.2;
    expect(evaluateQ26Manifest(bad).passed).toBe(false);
  });
});

describe("router + canary", () => {
  const bank = "bank.retry";
  const manifest = () => { const { tune, cert } = split(bank); const rep = runQ26(tune, cert); return { rep, m: rep.bindings[0].manifest! }; };
  const provider = () => new FixtureReadoutProvider("backend.t", [0.999, 0.001], deployment);
  const r = () => ({ ...req(bank), extensions: { "x-incumbent": "retry", "x-criteria": { true: "t" } } }) as DecisionRequestV1;

  test("live Q26 manifest without a canary never serves AUTO", async () => {
    const { m } = manifest();
    const router = new DecisionRouter({ provider: provider(), manifests: [m], mode: "live" });
    const out = await router.decide(r(), "state", { reversible: true });
    expect(out.calibration_level_served).toBe("L1");
    expect(out.threshold_action).toBe("REVIEW");
    expect((out.extensions as any)["x-reasons"].join()).toContain("canary");
  });
  test("canary: refuses ineligible banks, serves within the daily budget, audits every live auto-act, logs incumbent", async () => {
    const { rep, m } = manifest();
    const dir = tmp();
    const c = new CanaryController(join(dir, "canary.json"), () => new Date("2026-10-02T10:00:00Z"), () => 0.99);
    expect(() => c.enable("bank.other", rep)).toThrow(/not eligible/);
    c.enable(bank, rep, { budget_per_day: 2 });
    const ledger = new ShadowLedger(join(dir, "shadow"));
    const router = new DecisionRouter({ provider: provider(), manifests: [m], mode: "live", canary: c, ledger });
    const acts = [];
    for (let i = 0; i < 3; i++) acts.push(await router.decide(r(), "state", { reversible: true }));
    expect(acts.map((a) => a.threshold_action)).toEqual(["AUTO", "AUTO", "REVIEW"]);
    expect((acts[0].extensions as any)["x-audit"]).toBe(true);
    expect((acts[2].extensions as any)["x-reasons"].join()).toContain("exposure budget exhausted");
    await new Promise((res) => setTimeout(res, 10));
    const logged = readFileSync(join(dir, "shadow", "decisions", `${new Date().toISOString().slice(0, 10)}.jsonl`), "utf8").trim().split("\n").map((l) => JSON.parse(l));
    expect(logged[0]).toMatchObject({ served_live: true, audit: true, incumbent: "retry", criteria: { true: "t" } });
    expect(logged[2].served_live).toBeUndefined();
    // State persists across restarts.
    expect(new CanaryController(join(dir, "canary.json")).bank(bank)!.used.n).toBe(2);
  });
  test("CUSUM rolls a bad canary back to shadow automatically; a clean one keeps going and can grow", () => {
    const { rep } = manifest();
    const bad = new CanaryController(null, () => new Date("2026-10-02T00:00:00Z"));
    bad.enable(bank, rep);
    const r1 = rng(1);
    let rolledAt = -1;
    for (let i = 0; i < 400 && rolledAt < 0; i++) if (bad.recordAudit(bank, `d${i}`, r1() < 0.2)) rolledAt = i;
    expect(rolledAt).toBeGreaterThan(0);
    expect(bad.bank(bank)!.status).toBe("rolled_back");
    expect(bad.admit(bank).live).toBe(false);

    const good = new CanaryController(null, () => new Date("2026-10-02T00:00:00Z"));
    good.enable(bank, rep, { budget_per_day: 10 });
    expect(() => good.grow(bank)).toThrow(/audits/);
    const r2 = rng(2);
    for (let i = 0; i < 300; i++) good.recordAudit(bank, `d${i}`, r2() < 0.02);
    expect(good.bank(bank)!.status).toBe("canary");
    expect(good.grow(bank).budget_per_day).toBe(20);
    // Audit slice rate is pinned to 2–5%.
    expect(() => good.enable(bank, rep, { audit_rate: 0.1 })).toThrow(/0.02/);
  });
  test("sync feeds audited ledger rows (decision + outcome) into the CUSUM once", () => {
    const { rep } = manifest();
    const c = new CanaryController(null);
    c.enable(bank, rep);
    const rs = rows(bank, 50, { hiAcc: 0.5, share: 1 }).map((x) => ({ ...x, audit: true }));
    const first = c.syncFromRows(rs);
    expect(first.audited).toBe(50);
    expect(c.syncFromRows(rs).audited).toBe(0);
    expect(first.rollbacks).toEqual([bank]);
  });
});

describe("ledger export", () => {
  test("joins labels, drops rows without raw state, splits per (bank, type, k) with a fixed audit slice", async () => {
    const dir = tmp();
    const ledger = new ShadowLedger(dir);
    const prev = process.env.SYSTEM_ONE_SHADOW_STATE;
    const ids: string[] = [];
    for (let i = 0; i < 120; i++) {
      process.env.SYSTEM_ONE_SHADOW_STATE = i < 100 ? "1" : "0";
      const id = ledger.logDecision({
        primitive_id: "bank.err", operation: "GATE", question_id: "q", instructions: "is this retryable", subject_ref: null, state: `log line ${i}`,
        candidates: [], options: ["true", "false"], shape: "categorical", probs: [0.7, 0.3], readout_method: "fixture",
        scope: scopeFor("bank.err"), mode: "shadow", ts: new Date(Date.UTC(2026, 9, 1) + i * 1000).toISOString(), criteria: { true: "retry helps" },
      });
      ids.push(id);
    }
    process.env.SYSTEM_ONE_SHADOW_STATE = prev;
    await new Promise((res) => setTimeout(res, 20));
    ids.forEach((id, i) => ledger.recordOutcome({ decision_id: id, truth: i % 3 ? "true" : "false", source: "verifier:test" }));
    const { rows: out, summary } = buildExport(dir);
    expect(summary.labelled).toBe(120);
    expect(summary.dropped.no_state).toBe(20);
    expect(summary.kept).toBe(100);
    const audit = out.filter((r) => r.split === "audit");
    expect(audit.length).toBe(ids.slice(0, 100).filter((id) => unitHash(id) < 0.05).length);
    const g = summary.groups[0];
    expect(g).toMatchObject({ bank: "bank.err", type: "noul", k: 2 });
    expect(g.train + g.tune + g.cert + g.audit).toBe(100);
    // Certification is the newest slice; training never sees it.
    const train = out.filter((r) => r.split === "train"), cert = out.filter((r) => r.split === "cert");
    expect(train.at(-1)!.ts < cert[0].ts).toBe(true);
    // Noul label maps into the head's [false, true] order; criteria carried through.
    const t = out.find((r) => r.label === "true")!;
    expect(t.laya).toMatchObject({ question: { type: "noul", criteria: { true: "retry helps" } }, label_index: 1, option_order: [1, 0] });
    const o = join(dir, "export");
    writeExport(o, out, summary);
    expect(readFileSync(join(o, "train.jsonl"), "utf8").trim().split("\n").length).toBe(g.train);
  });
  test("choice rows map through candidate order; oversize menus and independent ops are dropped, not guessed", () => {
    const base = rows("bank.c", 1)[0];
    const ch = layaQuestion({ ...base, options: ["stop", "retry"], label_index: 0 } as DatasetRow);
    expect("q" in ch && ch.q.label_index).toBe(1);
    const big = { ...base, candidates: Array.from({ length: 17 }, (_, i) => ({ candidate_id: `c${i}`, label: null })) } as DatasetRow;
    expect(layaQuestion(big)).toEqual({ drop: "too_many_options" });
    expect(layaQuestion({ ...base, operation: "SUBSET" } as DatasetRow)).toEqual({ drop: "unsupported_operation" });
  });
});

describe("Laya revision identity (Q29: every revision is a new backend)", () => {
  test("env path > env revision > config.json > pin; fine-tuned dirs report their revision", () => {
    const d = tmp();
    const ck = join(d, "ckpt");
    mkdirSync(ck);
    writeFileSync(join(ck, "allternit_checkpoint.json"), JSON.stringify({ revision: "ft-abc" }));
    expect(layaRevision({ ALLTERNIT_LAYA_CHECKPOINT_PATH: ck, ALLTERNIT_LAYA_HOME: d })).toBe("ft-abc");
    expect(layaRevision({ ALLTERNIT_LAYA_REVISION: "deadbeef", ALLTERNIT_LAYA_HOME: d })).toBe("deadbeef");
    expect(layaRevision({ ALLTERNIT_LAYA_HOME: d })).toBe(LAYA_PINNED_REVISION);
    writeFileSync(join(d, "config.json"), JSON.stringify({ checkpoint: { path: ck } }));
    expect(layaRevision({ ALLTERNIT_LAYA_HOME: d })).toBe("ft-abc");
    writeFileSync(join(d, "config.json"), JSON.stringify({ checkpoint: { path: join(d, "other") } }));
    expect(layaRevision({ ALLTERNIT_LAYA_HOME: d })).toBe("local:other");
  });
});
