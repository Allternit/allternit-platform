// Q26 rollout after the gate: shadow -> canary with an exposure budget -> automatic
// rollback on a permanent random audit slice -> grow the budget.
//
// Per bank:
//   - status "shadow" (default) never serves live; "canary" serves live at most
//     `budget_per_day` auto-acts per UTC day; "rolled_back" is shadow until a human re-enables.
//   - every live auto-act is audited afterwards; on top, a permanent random audit
//     slice (2–5%) of all decisions is labelled.
//   - a Bernoulli CUSUM on audited errors (p0 = ε acceptable, p1 = 2ε) triggers an
//     automatic rollback to shadow when the statistic crosses h.
//   - the budget grows only by explicit call, and only after enough clean audits.
// State is one small JSON file (atomic writes). Enabling requires a Q26 report
// that marks the bank eligible.
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import type { Q26Report } from "./q26.ts";
import type { DatasetRow } from "./shadow.ts";

export type CanaryStatus = "shadow" | "canary" | "rolled_back";
export interface CanaryBank {
  status: CanaryStatus;
  budget_per_day: number;
  used: { day: string; n: number };
  audit_rate: number;
  epsilon: number;
  /** CUSUM threshold (log-likelihood units). */
  h: number;
  cusum: number;
  audits: number; audit_errors: number; audits_since_grow: number;
  processed: string[];
  events: { ts: string; event: string; detail?: string }[];
}
export interface CanaryState { banks: Record<string, CanaryBank> }
export interface Admission { live: boolean; audit: boolean; reason: string }

export const CANARY_DEFAULTS = { audit_rate: 0.03, h: 4.0, budget_per_day: 20, grow_min_audits: 50, grow_factor: 2, max_processed: 5000 } as const;

const day = (d: Date) => d.toISOString().slice(0, 10);

export class CanaryController {
  private state: CanaryState;
  constructor(readonly path: string | null, private now: () => Date = () => new Date(), private rand: () => number = Math.random) {
    this.state = path && existsSync(path) ? (JSON.parse(readFileSync(path, "utf8")) as CanaryState) : { banks: {} };
  }

  private save() {
    if (!this.path) return;
    mkdirSync(dirname(this.path), { recursive: true });
    const tmp = `${this.path}.${process.pid}.tmp`;
    writeFileSync(tmp, JSON.stringify(this.state, null, 2) + "\n", { mode: 0o600 });
    renameSync(tmp, this.path);
  }
  private event(b: CanaryBank, event: string, detail?: string) {
    b.events.push({ ts: this.now().toISOString(), event, ...(detail ? { detail } : {}) });
    if (b.events.length > 200) b.events.splice(0, b.events.length - 200);
  }

  bank(name: string): CanaryBank | undefined { return this.state.banks[name]; }
  snapshot(): CanaryState { return JSON.parse(JSON.stringify(this.state)); }

  /** Shadow -> canary. Refused unless the Q26 report marks the bank eligible. */
  enable(name: string, report: Q26Report, o: { budget_per_day?: number; audit_rate?: number; h?: number } = {}): CanaryBank {
    const r = report.banks[name];
    if (report.gate !== "Q26" || !r?.eligible || r.epsilon == null) throw new Error(`bank ${name} is not eligible under Q26; it stays in shadow`);
    const audit = o.audit_rate ?? CANARY_DEFAULTS.audit_rate;
    if (!(audit >= 0.02 && audit <= 0.05)) throw new Error("audit_rate must be within 0.02–0.05 (Q26)");
    const budget = o.budget_per_day ?? CANARY_DEFAULTS.budget_per_day;
    if (!(Number.isInteger(budget) && budget > 0)) throw new Error("budget_per_day must be a positive integer");
    const prev = this.state.banks[name];
    const b: CanaryBank = {
      status: "canary", budget_per_day: budget, used: { day: day(this.now()), n: 0 }, audit_rate: audit, epsilon: r.epsilon,
      h: o.h ?? CANARY_DEFAULTS.h, cusum: 0, audits: 0, audit_errors: 0, audits_since_grow: 0,
      processed: prev?.processed ?? [], events: prev?.events ?? [],
    };
    this.event(b, "enabled", `budget ${budget}/day, ε ${r.epsilon}, audit ${audit}`);
    this.state.banks[name] = b;
    this.save();
    return b;
  }

  /** May this S1 auto-act be served live? Every admitted one is audited afterwards. */
  admit(name: string): Admission {
    const b = this.state.banks[name];
    if (!b || b.status !== "canary") return { live: false, audit: this.sampleAudit(name), reason: b?.status === "rolled_back" ? "canary rolled back: shadow only" : "bank not in canary: shadow only" };
    const today = day(this.now());
    if (b.used.day !== today) b.used = { day: today, n: 0 };
    if (b.used.n >= b.budget_per_day) { this.save(); return { live: false, audit: this.sampleAudit(name), reason: `exposure budget exhausted (${b.budget_per_day}/day)` }; }
    b.used.n++;
    this.save();
    return { live: true, audit: true, reason: "canary: within exposure budget" };
  }

  /** Permanent random audit slice (applies in every status). */
  sampleAudit(name: string): boolean {
    const rate = this.state.banks[name]?.audit_rate ?? CANARY_DEFAULTS.audit_rate;
    return this.rand() < rate;
  }

  /** One audited decision. Returns true when this audit triggered the rollback. */
  recordAudit(name: string, decisionId: string, wrong: boolean): boolean {
    const b = this.state.banks[name];
    if (!b || b.processed.includes(decisionId)) return false;
    b.processed.push(decisionId);
    if (b.processed.length > CANARY_DEFAULTS.max_processed) b.processed.splice(0, b.processed.length - CANARY_DEFAULTS.max_processed);
    b.audits++; b.audits_since_grow++;
    if (wrong) b.audit_errors++;
    const p0 = b.epsilon, p1 = Math.min(2 * b.epsilon, 0.5);
    const llr = wrong ? Math.log(p1 / p0) : Math.log((1 - p1) / (1 - p0));
    b.cusum = Math.max(0, b.cusum + llr);
    let rolled = false;
    if (b.status === "canary" && b.cusum >= b.h) {
      b.status = "rolled_back"; rolled = true;
      this.event(b, "rollback", `CUSUM ${b.cusum.toFixed(2)} ≥ h ${b.h} after ${b.audits} audits (${b.audit_errors} errors)`);
    }
    this.save();
    return rolled;
  }

  /** Feed audited ledger rows (decision joined with its outcome) into the CUSUM. */
  syncFromRows(rows: DatasetRow[]): { audited: number; rollbacks: string[] } {
    let audited = 0;
    const rollbacks: string[] = [];
    for (const r of [...rows].sort((a, b) => a.ts.localeCompare(b.ts))) {
      if (!(r.audit || r.served_live) || !this.state.banks[r.primitive_id]) continue;
      if (this.state.banks[r.primitive_id].processed.includes(r.decision_id)) continue;
      const p = r.readout.probs;
      const wrong = p.indexOf(Math.max(...p)) !== r.label_index;
      audited++;
      if (this.recordAudit(r.primitive_id, r.decision_id, wrong)) rollbacks.push(r.primitive_id);
    }
    return { audited, rollbacks };
  }

  /** Grow the exposure budget after enough clean audits; refused while the CUSUM is elevated. */
  grow(name: string, factor: number = CANARY_DEFAULTS.grow_factor): CanaryBank {
    const b = this.state.banks[name];
    if (!b || b.status !== "canary") throw new Error(`bank ${name} is not in canary`);
    if (b.audits_since_grow < CANARY_DEFAULTS.grow_min_audits) throw new Error(`need ${CANARY_DEFAULTS.grow_min_audits} audits since the last change (have ${b.audits_since_grow})`);
    if (b.cusum >= b.h / 2) throw new Error(`CUSUM ${b.cusum.toFixed(2)} is elevated; not growing`);
    b.budget_per_day = Math.ceil(b.budget_per_day * factor);
    b.audits_since_grow = 0;
    this.event(b, "grow", `budget ${b.budget_per_day}/day`);
    this.save();
    return b;
  }

  rollback(name: string, why = "manual"): void {
    const b = this.state.banks[name];
    if (!b) return;
    b.status = "rolled_back";
    this.event(b, "rollback", why);
    this.save();
  }
}
