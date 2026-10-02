// Ledger export -> fine-tuning set (WP-L1). Joins shadow decisions with their
// outcome labels (harvest), keeps only rows that carry raw state (Q28 opt-in),
// rebuilds the exact typed question the S1 head saw, and splits per
// (bank, type, option count):
//   audit  a permanent random slice (hash of decision_id): never trained, tuned or certified on
//   train  oldest rows: fine-tuning only
//   tune   next rows: Q26 split A (temperature + threshold)
//   cert   newest rows: Q26 split B (untouched certification)
// Nothing here invents a label or a state: a decision without both is dropped and counted.
// Q26 (WP-L2): teacher labels (model judgements) are training rows ONLY. They never enter
// split A (tune), split B (cert) or the audit slice, and never count toward Q26 minimums.
import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { harvest, type DatasetRow, type HarvestStats, type LabelSource } from "./shadow.ts";

export type LayaType = "noul" | "choice" | "score";
export interface ExportRow extends DatasetRow {
  state: string;
  split: "train" | "tune" | "cert" | "audit";
  /** The question as the S1 head receives it, plus the label in the head's own option order. */
  laya: { question: { type: LayaType; instructions: string; criteria?: unknown }; label_index: number; option_order: number[] };
}
export interface ExportOptions {
  primitive?: string; model?: string;
  /** Permanent audit slice share, 0.02–0.05 (Q26). Default 0.05. */
  auditFraction?: number;
  /** Shares of the non-audit rows, oldest first. Default 0.5 / 0.2 / 0.3. */
  fractions?: { train: number; tune: number; cert: number };
  /** The direct option limit of the head; larger menus are served coarse-to-fine and are not trained on directly. */
  maxOptions?: number;
  /** More ledgers in the shadow layout (e.g. the label-harvest dir). */
  extraDirs?: string[];
}
/** Q26 sizing: ~500–1,100 certification + 300–500 tuning rows per bank (real labels only). */
export const Q26_MIN = { cert: 500, tune: 300 } as const;
export interface ExportGroupStats {
  bank: string; type: string; k: number; train: number; tune: number; cert: number; audit: number;
  by_label_source: Record<LabelSource, number>;
  /** Real (non-teacher) rows in tune/cert reach Q26_MIN. */
  meets_q26_minimums: boolean;
}
export interface ExportSummary {
  harvest: HarvestStats; labelled: number; kept: number;
  dropped: { no_state: number; unsupported_operation: number; too_many_options: number; bad_options: number };
  groups: ExportGroupStats[];
}

const NOUL_OPS = new Set(["BELIEF", "GATE", "VERIFY"]);
const CHOICE_OPS = new Set(["CHOICE", "RANK", "ROUTE"]);
const SCORE_OPS = new Set(["SCORE", "ESTIMATE"]);

/** Deterministic [0,1) from an id, so the audit slice never moves between exports. */
export const unitHash = (id: string) => parseInt(createHash("sha256").update(`audit:${id}`).digest("hex").slice(0, 12), 16) / 2 ** 48;

/** Rebuild the head's question for a ledger row; null when the row cannot be trained on as-is. */
export function layaQuestion(r: DatasetRow, maxOptions = 16): { q: ExportRow["laya"] } | { drop: keyof ExportSummary["dropped"] } {
  const op = r.operation.toUpperCase();
  if (NOUL_OPS.has(op)) {
    if (r.options.length !== 2 || !r.options.includes("true") || !r.options.includes("false")) return { drop: "bad_options" };
    // The head's noul order is [false, true].
    const order = r.options.map((o) => (o === "true" ? 1 : 0));
    return { q: { question: { type: "noul", instructions: r.question.instructions, ...(r.criteria ? { criteria: r.criteria } : {}) }, label_index: order[r.label_index], option_order: order } };
  }
  if (CHOICE_OPS.has(op)) {
    const ids = r.candidates.map((c) => c.candidate_id);
    if (ids.length < 2) return { drop: "bad_options" };
    if (ids.length > maxOptions) return { drop: "too_many_options" };
    const order = r.options.map((o) => ids.indexOf(o));
    if (order.some((i) => i < 0)) return { drop: "bad_options" };
    const criteria = Object.fromEntries(r.candidates.map((c) => [c.candidate_id, c.label ?? c.candidate_id]));
    return { q: { question: { type: "choice", instructions: r.question.instructions, criteria }, label_index: order[r.label_index], option_order: order } };
  }
  if (SCORE_OPS.has(op)) {
    if (r.options.length > maxOptions) return { drop: "too_many_options" };
    const levels = r.scale && r.scale.length === r.options.length ? r.scale : r.options;
    return { q: { question: { type: "score", instructions: r.question.instructions, criteria: levels }, label_index: r.label_index, option_order: r.options.map((_, i) => i) } };
  }
  return { drop: "unsupported_operation" };
}

export function buildExport(shadowDir: string, o: ExportOptions = {}): { rows: ExportRow[]; summary: ExportSummary } {
  const audit = o.auditFraction ?? 0.05;
  if (!(audit >= 0.02 && audit <= 0.05)) throw new Error("auditFraction must be within 0.02–0.05 (Q26)");
  const fr = o.fractions ?? { train: 0.5, tune: 0.2, cert: 0.3 };
  const tot = fr.train + fr.tune + fr.cert;
  const { rows, stats } = harvest(shadowDir, { primitive: o.primitive, model: o.model, extraDirs: o.extraDirs });
  const summary: ExportSummary = { harvest: stats, labelled: rows.length, kept: 0, dropped: { no_state: 0, unsupported_operation: 0, too_many_options: 0, bad_options: 0 }, groups: [] };
  const groups = new Map<string, ExportRow[]>();
  const out: ExportRow[] = [];
  for (const r of rows) {
    if (typeof r.state !== "string" || !r.state.length) { summary.dropped.no_state++; continue; }
    const lq = layaQuestion(r, o.maxOptions);
    if ("drop" in lq) { summary.dropped[lq.drop]++; continue; }
    const row: ExportRow = { ...r, state: r.state, split: "train", laya: lq.q };
    if (r.provenance.label_source === "teacher") { out.push(row); continue; } // Q26: tuning data only
    if (unitHash(r.decision_id) < audit) { row.split = "audit"; out.push(row); }
    const key = `${r.primitive_id}\u0000${lq.q.question.type}\u0000${r.options.length}`;
    if (row.split !== "audit") { if (!groups.has(key)) groups.set(key, []); groups.get(key)!.push(row); }
  }
  const gstats = new Map<string, ExportGroupStats>();
  const statFor = (r: ExportRow) => {
    const k = `${r.primitive_id}\u0000${r.laya.question.type}\u0000${r.options.length}`;
    if (!gstats.has(k)) gstats.set(k, { bank: r.primitive_id, type: r.laya.question.type, k: r.options.length, train: 0, tune: 0, cert: 0, audit: 0, by_label_source: { observed: 0, backfill_observed: 0, teacher: 0 }, meets_q26_minimums: false });
    return gstats.get(k)!;
  };
  for (const rs of groups.values()) {
    rs.sort((a, b) => a.ts.localeCompare(b.ts));
    const nCert = Math.floor((rs.length * fr.cert) / tot), nTune = Math.floor((rs.length * fr.tune) / tot);
    const nTrain = rs.length - nCert - nTune;
    rs.forEach((r, i) => { r.split = i < nTrain ? "train" : i < nTrain + nTune ? "tune" : "cert"; out.push(r); });
  }
  for (const r of out) { const g = statFor(r); g[r.split]++; g.by_label_source[r.provenance.label_source ?? "observed"]++; }
  for (const g of gstats.values()) g.meets_q26_minimums = g.cert >= Q26_MIN.cert && g.tune >= Q26_MIN.tune;
  out.sort((a, b) => a.ts.localeCompare(b.ts));
  summary.kept = out.length;
  summary.groups = [...gstats.values()].sort((a, b) => `${a.bank}${a.type}${a.k}`.localeCompare(`${b.bank}${b.type}${b.k}`));
  return { rows: out, summary };
}

/** Writes train/tune/cert/audit.jsonl + summary.json (0600: rows hold raw user text). */
export function writeExport(dir: string, rows: ExportRow[], summary: ExportSummary) {
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  for (const split of ["train", "tune", "cert", "audit"] as const) {
    const part = rows.filter((r) => r.split === split);
    writeFileSync(join(dir, `${split}.jsonl`), part.map((r) => JSON.stringify(r)).join("\n") + (part.length ? "\n" : ""), { mode: 0o600 });
  }
  writeFileSync(join(dir, "summary.json"), JSON.stringify(summary, null, 2) + "\n", { mode: 0o600 });
}
