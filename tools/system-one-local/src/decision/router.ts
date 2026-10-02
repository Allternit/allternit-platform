// Decision router. S1 is SHADOW by default and the router REFUSES uncalibrated S1:
// without a bound, gate-passing calibration manifest the result is UNCALIBRATED,
// abstained, and can never be AUTO. Hard policy and deterministic verification
// stay authoritative; this router only ever proposes.
import type { CalibrationScope, DecisionCalibrationManifestV1, DecisionRequestV1, DecisionResultV1, ThresholdAction } from "./contract.ts";
import type { CanaryController } from "./canary.ts";
import { evaluateQ22Gate, type GateOptions } from "./gate.ts";
import { evaluateQ26Manifest } from "./q26.ts";
import { candidateSchemaHash, candidateSetHash, checkBinding } from "./manifest.ts";
import { calibrateProbs, shapeAnswer, type DecisionReadoutProvider } from "./readout.ts";
import type { ShadowLedger } from "./shadow.ts";
import { actionFor, DEFAULT_PROFILE, validateProfile, type ThresholdProfile } from "./threshold.ts";

export type RouterMode = "shadow" | "live";

export interface RouteContext {
  /** Caller attests the decision is reversible / low-consequence. Default false => never AUTO. */
  reversible?: boolean;
  /** Harvest replay only: a stable decision id and the historic event time, so a re-run never duplicates a row. */
  decisionId?: string;
  ts?: string;
}

export interface RouterConfig {
  provider: DecisionReadoutProvider;
  manifests: DecisionCalibrationManifestV1[];
  profiles?: ThresholdProfile[];
  /** Default "shadow". "live" still needs a passing per-primitive manifest to ever emit AUTO. */
  mode?: RouterMode;
  primitiveId?: string;
  gate?: GateOptions;
  /** When set, every decision appends a shadow record (raw readout, scope) for later harvest + calibration. */
  ledger?: ShadowLedger;
  /** Q26 rollout: exposure budget, audit slice, CUSUM rollback. Q26 manifests only serve live through it. */
  canary?: CanaryController;
}

export class DecisionRouter {
  readonly mode: RouterMode;
  constructor(private cfg: RouterConfig) {
    this.mode = cfg.mode ?? "shadow";
    for (const p of cfg.profiles ?? []) {
      const e = validateProfile(p);
      if (e.length) throw new Error(`invalid threshold profile ${p.profile_id}: ${e.join("; ")}`);
    }
  }

  async decide(req: DecisionRequestV1, state: string, ctx: RouteContext = {}): Promise<DecisionResultV1> {
    const raw = await this.cfg.provider.readout(req, state);
    const profile = (this.cfg.profiles ?? []).find((p) => p.profile_id === req.threshold_profile_id) ?? DEFAULT_PROFILE;
    const scope: CalibrationScope = {
      ...raw.deployment,
      question_id: req.question_id ?? "",
      candidate_schema_hash: candidateSchemaHash(req),
      candidate_set_hash: candidateSetHash(req),
      threshold_profile: profile.profile_id,
    };
    const reasons: string[] = [];
    const served = this.pickManifest(scope, req, raw.options.length, reasons);

    let probs = raw.probs;
    let semantics: DecisionResultV1["confidence_semantics"] = "UNCALIBRATED";
    let level: DecisionResultV1["calibration_level_served"] = "NONE";
    let action: ThresholdAction = "REVIEW"; // uncalibrated: never trust the numbers, never AUTO
    let abstained = true;
    const primitive_id = String(req.extensions?.["x-primitive_id"] ?? this.cfg.primitiveId ?? req.decision_bank_id);
    let servedLive = false;
    let audit: boolean | null = null;
    if (served) {
      probs = calibrateProbs(raw.probs, served.extensions?.["x-temperature"] ?? 1, raw.shape);
      semantics = "CALIBRATED"; level = "L1"; abstained = false;
      const conf = shapeAnswer(req.operation, raw.options, probs, req.scale).confidence;
      action = actionFor(profile, conf);
      if (action === "AUTO") {
        if (this.mode !== "live") { action = "REVIEW"; reasons.push("shadow mode: AUTO downgraded to REVIEW"); }
        else if (ctx.reversible !== true) { action = "REVIEW"; reasons.push("not attested reversible/low-consequence: AUTO downgraded"); }
        else if (conf < (served.extensions?.["x-auto_min_confidence"] ?? 1)) { action = "REVIEW"; reasons.push("confidence below the calibrated auto-act region"); }
        else if (this.cfg.canary) {
          const adm = this.cfg.canary.admit(primitive_id);
          audit = adm.audit;
          if (adm.live) servedLive = true; else { action = "REVIEW"; reasons.push(adm.reason); }
        } else if (served.extensions?.["x-gate"] === "Q26") { action = "REVIEW"; reasons.push("Q26: live serving only through a canary exposure budget"); }
        else servedLive = true;
      }
    }
    const shaped = shapeAnswer(req.operation, raw.options, probs, req.scale);
    if (audit === null) audit = this.cfg.canary?.sampleAudit(primitive_id) ?? false;
    const ext = req.extensions ?? {};
    const decision_id = this.cfg.ledger?.logDecision({
      primitive_id, operation: req.operation, question_id: req.question_id ?? "", instructions: req.instructions,
      subject_ref: typeof req.extensions?.["x-subject_ref"] === "string" ? (req.extensions["x-subject_ref"] as string) : null,
      state, candidates: (req.candidates ?? []).map((c) => ({ candidate_id: c.candidate_id, label: c.label ?? null })),
      options: raw.options, shape: raw.shape ?? "categorical", probs: raw.probs, readout_method: raw.method, scope, mode: this.mode,
      ...(ext["x-criteria"] && typeof ext["x-criteria"] === "object" ? { criteria: ext["x-criteria"] as Record<string, string> } : {}),
      ...(req.scale ? { scale: req.scale as unknown[] } : {}),
      ...(typeof ext["x-incumbent"] === "string" ? { incumbent: ext["x-incumbent"] as string } : {}),
      ...(servedLive ? { served_live: true } : {}), ...(audit ? { audit: true } : {}),
      ...(ctx.decisionId ? { decision_id: ctx.decisionId } : {}), ...(ctx.ts ? { ts: ctx.ts } : {}),
    });
    return {
      envelope: { ...req.envelope, schema_id: "allternit.kernel.DecisionResultV1" },
      operation: req.operation,
      answer: shaped.answer,
      probabilities: Object.fromEntries(raw.options.map((o, i) => [o, probs[i]])),
      confidence: shaped.confidence,
      confidence_semantics: semantics,
      calibration_level_served: level,
      calibration_id: served?.manifest_id ?? null,
      calibration_fingerprint: served?.scope_fingerprint ?? null,
      evidence_refs: [],
      backend_id: this.cfg.provider.backend_id,
      threshold_action: action,
      latency_ms: raw.latency_ms,
      abstained,
      extensions: { ...(raw.usage ? { "x-usage": raw.usage } : {}), "x-mode": this.mode, "x-readout_kind": raw.kind, "x-readout_method": raw.method, "x-refused_uncalibrated": !served, ...(decision_id ? { "x-decision_id": decision_id } : {}), "x-reasons": reasons, "x-served_live": servedLive, "x-audit": audit },
    };
  }

  private pickManifest(scope: CalibrationScope, req: DecisionRequestV1, nOptions: number, reasons: string[]) {
    let sawAny = false;
    for (const m of this.cfg.manifests) {
      if (this.cfg.primitiveId && m.primitive_id !== this.cfg.primitiveId) continue;
      sawAny = true;
      const b = checkBinding(m, scope);
      if (!b.ok) { reasons.push(`manifest ${m.manifest_id}: ${b.reason}`); continue; }
      const g = m.extensions?.["x-gate"] === "Q26" ? evaluateQ26Manifest(m) : evaluateQ22Gate(m, this.cfg.gate);
      if (!g.passed || m.gate.passed !== true) { reasons.push(`manifest ${m.manifest_id}: gate failed (${g.failures.join("; ") || "manifest.gate.passed=false"})`); continue; }
      const cr = m.coverage_region;
      if (cr?.min_candidates !== undefined && nOptions < cr.min_candidates) { reasons.push("outside calibrated coverage: too few candidates"); continue; }
      if (cr?.max_candidates !== undefined && nOptions > cr.max_candidates) { reasons.push("outside calibrated coverage: too many candidates"); continue; }
      if (cr?.calibration_domain && req.calibration_domain !== cr.calibration_domain) { reasons.push("outside calibrated coverage: calibration_domain"); continue; }
      return m;
    }
    if (!sawAny) reasons.push("no calibration manifest for this primitive: S1 refused, shadow only");
    return null;
  }
}
