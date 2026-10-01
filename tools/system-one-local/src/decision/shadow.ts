// Shadow ledger + harvester. While S1 runs in shadow the router appends one
// record per decision (raw readout, scope, candidates; state is hashed, never
// stored). Ground truth comes LATER from deterministic code (parser, test run,
// verifier) via recordOutcome(). The harvester joins the two into a JSONL
// dataset that `system-one calibrate` consumes. Nothing here invents labels:
// a decision with no recorded outcome is never a row.
import { createHash, randomUUID } from "node:crypto";
import { appendFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { CalibrationScope } from "./contract.ts";

export interface ShadowDecisionRecord {
  kind: "decision";
  decision_id: string;
  ts: string;
  primitive_id: string;
  operation: string;
  question_id: string;
  instructions: string;
  /** Caller-supplied join key (request.extensions["x-subject_ref"]), e.g. a tool-call or test-run id. */
  subject_ref: string | null;
  state_sha256: string;
  candidates: { candidate_id: string; label: string | null }[];
  options: string[];
  shape: "categorical" | "independent";
  /** RAW (uncalibrated) probabilities aligned with options. Calibration is fit later, offline. */
  probs: number[];
  readout_method: string;
  scope: CalibrationScope;
  mode: string;
  /** Noul criteria (x-criteria) / score scale: needed to rebuild the exact question for fine-tuning. */
  criteria?: Record<string, string> | null;
  scale?: unknown[] | null;
  /** The incumbent decider's answer (x-incumbent), for Q26 non-inferiority. */
  incumbent?: string | null;
  /** Canary: S1's answer was acted on live, and/or this decision is in the audit slice. */
  served_live?: boolean;
  audit?: boolean;
  /** Raw state, only with SYSTEM_ONE_SHADOW_STATE=1 (Q28 opt-in). */
  state?: string;
}

export interface OutcomeRecord {
  kind: "outcome";
  ts: string;
  /** Join by decision_id (exact) or by subject_ref (+ optional question_id). */
  decision_id?: string | null;
  subject_ref?: string | null;
  question_id?: string | null;
  /** The option/candidate id that deterministic code established as correct. */
  truth: string;
  /** Who established it, e.g. "verifier:tests", "parser:tsc". Mandatory: no anonymous labels. */
  source: string;
}

export interface DatasetRow {
  decision_id: string;
  ts: string;
  primitive_id: string;
  operation: string;
  question: { question_id: string; instructions: string };
  candidates: { candidate_id: string; label: string | null }[];
  options: string[];
  readout: { probs: number[]; method: string; probs_flipped?: number[]; probs_reordered?: number[] };
  label: string;
  label_index: number;
  scope: CalibrationScope;
  state?: string;
  criteria?: Record<string, string> | null;
  scale?: unknown[] | null;
  incumbent?: string | null;
  served_live?: boolean;
  audit?: boolean;
  provenance: { decision_log: string; outcome_source: string; outcome_ts: string; state_sha256: string; subject_ref: string | null };
}

export const sha256Hex = (s: string) => createHash("sha256").update(s).digest("hex");
const day = (ts: string) => ts.slice(0, 10);

export class ShadowLedger {
  constructor(readonly dir: string) {}

  private append(sub: string, ts: string, rec: unknown) {
    const d = join(this.dir, sub);
    mkdirSync(d, { recursive: true, mode: 0o700 });
    appendFileSync(join(d, `${day(ts)}.jsonl`), `${JSON.stringify(rec)}\n`, { mode: 0o600 });
  }

  /** Off the hot path: deferred, and never throws into a decision. */
  logDecision(rec: Omit<ShadowDecisionRecord, "kind" | "decision_id" | "ts" | "state_sha256"> & { state: string; decision_id?: string; ts?: string }): string {
    const { state, ...rest } = rec;
    const full: ShadowDecisionRecord = { kind: "decision", decision_id: rec.decision_id ?? randomUUID(), ts: rec.ts ?? new Date().toISOString(), ...rest, state_sha256: sha256Hex(state) };
    // Q28: the raw state is kept only on opt-in (it is the training text for fine-tuning S1).
    if (process.env.SYSTEM_ONE_SHADOW_STATE === "1") (full as ShadowDecisionRecord & { state?: string }).state = state;
    queueMicrotask(() => { try { this.append("decisions", full.ts, full); } catch { /* logging never breaks a decision */ } });
    return full.decision_id;
  }

  /** Synchronous: outcomes are the valuable half and are written by batch jobs, not the hot path. */
  recordOutcome(o: Omit<OutcomeRecord, "kind" | "ts"> & { ts?: string }): OutcomeRecord {
    if (!o.source?.trim()) throw new Error("outcome needs a source (who established the ground truth)");
    if (!o.truth?.trim()) throw new Error("outcome needs a truth candidate id");
    if (!o.decision_id && !o.subject_ref) throw new Error("outcome needs decision_id or subject_ref");
    const full: OutcomeRecord = { kind: "outcome", ts: o.ts ?? new Date().toISOString(), ...o };
    this.append("outcomes", full.ts, full);
    return full;
  }
}

function readJsonl<T>(dir: string): { rec: T; file: string }[] {
  if (!existsSync(dir)) return [];
  const out: { rec: T; file: string }[] = [];
  for (const f of readdirSync(dir).filter((x) => x.endsWith(".jsonl")).sort()) {
    for (const line of readFileSync(join(dir, f), "utf8").split("\n")) {
      if (!line.trim()) continue;
      try { out.push({ rec: JSON.parse(line) as T, file: f }); } catch { /* skip torn line */ }
    }
  }
  return out;
}

export interface HarvestStats { decisions: number; outcomes: number; joined: number; no_outcome: number; unmatched_outcomes: number; skipped_independent: number; truth_not_in_options: number }

export function harvest(dir: string, opts: { primitive?: string; model?: string } = {}): { rows: DatasetRow[]; stats: HarvestStats } {
  const decisions = readJsonl<ShadowDecisionRecord>(join(dir, "decisions")).filter((d) => d.rec.kind === "decision");
  const outcomes = readJsonl<OutcomeRecord>(join(dir, "outcomes")).map((o) => o.rec).filter((o) => o.kind === "outcome");
  const stats: HarvestStats = { decisions: decisions.length, outcomes: outcomes.length, joined: 0, no_outcome: 0, unmatched_outcomes: 0, skipped_independent: 0, truth_not_in_options: 0 };
  const byId = new Map(decisions.map((d) => [d.rec.decision_id, d]));
  // Latest outcome wins per decision (deterministic re-runs supersede earlier ones).
  const chosen = new Map<string, OutcomeRecord>();
  for (const o of [...outcomes].sort((a, b) => a.ts.localeCompare(b.ts))) {
    let target: string | undefined;
    if (o.decision_id) target = byId.has(o.decision_id) ? o.decision_id : undefined;
    else if (o.subject_ref) {
      // Latest matching decision made at or before the outcome.
      const cands = decisions.filter((d) => d.rec.subject_ref === o.subject_ref && (!o.question_id || d.rec.question_id === o.question_id) && d.rec.ts <= o.ts);
      cands.sort((a, b) => a.rec.ts.localeCompare(b.rec.ts));
      target = cands.at(-1)?.rec.decision_id;
    }
    if (!target) { stats.unmatched_outcomes++; continue; }
    chosen.set(target, o);
  }
  const rows: DatasetRow[] = [];
  for (const { rec: d, file } of decisions) {
    if (opts.primitive && d.primitive_id !== opts.primitive) continue;
    if (opts.model && d.scope.model_ref !== opts.model) continue;
    const o = chosen.get(d.decision_id);
    if (!o) { stats.no_outcome++; continue; }
    if (d.shape !== "categorical") { stats.skipped_independent++; continue; }
    const li = d.options.indexOf(o.truth);
    if (li < 0) { stats.truth_not_in_options++; continue; }
    stats.joined++;
    rows.push({
      decision_id: d.decision_id, ts: d.ts, primitive_id: d.primitive_id, operation: d.operation,
      question: { question_id: d.question_id, instructions: d.instructions },
      candidates: d.candidates, options: d.options, readout: { probs: d.probs, method: d.readout_method },
      label: o.truth, label_index: li, scope: d.scope,
      ...(typeof d.state === "string" ? { state: d.state } : {}),
      ...(d.criteria ? { criteria: d.criteria } : {}), ...(d.scale ? { scale: d.scale } : {}),
      ...(d.incumbent != null ? { incumbent: d.incumbent } : {}),
      ...(d.served_live ? { served_live: true } : {}), ...(d.audit ? { audit: true } : {}),
      provenance: { decision_log: `decisions/${file}`, outcome_source: o.source, outcome_ts: o.ts, state_sha256: d.state_sha256, subject_ref: d.subject_ref },
    });
  }
  rows.sort((a, b) => a.ts.localeCompare(b.ts));
  return { rows, stats };
}

export function writeDataset(rows: DatasetRow[], path: string) {
  writeFileSync(path, rows.map((r) => JSON.stringify(r)).join("\n") + (rows.length ? "\n" : ""));
}
export function readDataset(path: string): DatasetRow[] {
  return readFileSync(path, "utf8").split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l) as DatasetRow);
}
