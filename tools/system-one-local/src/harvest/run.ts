// WP-L2 harvest runner: redact -> dedupe -> replay through the real router -> label.
// Writes to its own ledger dir (default ~/.allternit/system-one/harvest) in the
// shadow layout, which `system-one export` reads alongside the live shadow dir.
// Idempotent: decision_id = hash(bank, question, source, key), so a re-run skips
// every item already in the ledger and never duplicates a row.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import type { DecisionRequestV1 } from "../decision/contract.ts";
import type { DecisionRouter } from "../decision/router.ts";
import { sha256Hex, ShadowLedger } from "../decision/shadow.ts";
import { redact, type RedactionFlags } from "../hook/pack.ts";
import type { HarvestItem, HarvestSource } from "./types.ts";

/** Longest state the S1 head sees usefully (ModernBERT context); longer states are cut at the tail. */
export const MAX_STATE_CHARS = 6000;

export const harvestDecisionId = (it: Pick<HarvestItem, "spec" | "source" | "key">) => {
  const h = sha256Hex(`harvest\u0000${it.spec.primitive_id}\u0000${it.spec.question_id ?? ""}\u0000${it.source}\u0000${it.key}`);
  return `h-${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20, 32)}`;
};

export function redactState(text: string, flags: RedactionFlags = { secret: 0, email: 0, client: 0, possible_name: 0 }) {
  const out = redact(text, flags);
  return out.length > MAX_STATE_CHARS ? out.slice(0, MAX_STATE_CHARS) : out;
}

export function requestFor(it: HarvestItem, decisionId: string): DecisionRequestV1 {
  const s = it.spec;
  return {
    envelope: { harvest: true },
    operation: s.operation,
    state_projection_ref: `harvest:${decisionId}`,
    instructions: s.instructions,
    question_id: s.question_id,
    decision_bank_id: s.bank,
    ...(s.candidates ? { candidates: s.candidates } : {}),
    extensions: {
      "x-primitive_id": s.primitive_id,
      ...(s.criteria ? { "x-criteria": s.criteria } : {}),
      ...(it.incumbent ? { "x-incumbent": it.incumbent } : {}),
    },
  };
}

/** Decision ids already in a ledger dir (decisions/*.jsonl), plus those with an outcome. */
export function existingIds(dir: string): { decisions: Set<string>; outcomes: Set<string> } {
  const read = (sub: string, field: string) => {
    const s = new Set<string>();
    const d = join(dir, sub);
    if (!existsSync(d)) return s;
    for (const f of readdirSync(d).filter((x) => x.endsWith(".jsonl"))) {
      for (const line of readFileSync(join(d, f), "utf8").split("\n")) {
        if (!line.trim()) continue;
        try { const v = JSON.parse(line)[field]; if (typeof v === "string") s.add(v); } catch { /* torn line */ }
      }
    }
    return s;
  };
  return { decisions: read("decisions", "decision_id"), outcomes: read("outcomes", "decision_id") };
}

export interface HarvestRunStats {
  seen: number; replayed: number; skipped_existing: number; failed: number; capped: number;
  redactions: RedactionFlags;
  /** bank -> label_source -> count, for the rows this run wrote. */
  per_bank: Record<string, Record<string, number>>;
  errors: string[];
}

export interface HarvestRunOptions {
  dir: string;
  router: DecisionRouter;
  sources: HarvestSource[];
  /** Max NEW items per bank this run (Laya readouts cost wall time). */
  capPerBank?: number;
  concurrency?: number;
  dryRun?: boolean;
  log?: (s: string) => void;
}

export async function runHarvest(o: HarvestRunOptions): Promise<HarvestRunStats> {
  // Q28 opt-in: the harvest exists to produce training text, so the ledger keeps the (redacted) state.
  process.env.SYSTEM_ONE_SHADOW_STATE = "1";
  const ledger = new ShadowLedger(o.dir);
  const have = existingIds(o.dir);
  const stats: HarvestRunStats = { seen: 0, replayed: 0, skipped_existing: 0, failed: 0, capped: 0, redactions: { secret: 0, email: 0, client: 0, possible_name: 0 }, per_bank: {}, errors: [] };
  const newPerBank = new Map<string, number>();
  const queue: { it: HarvestItem; id: string; state: string }[] = [];
  const seenIds = new Set<string>();
  for (const src of o.sources) {
    for await (const it of src.items() as AsyncIterable<HarvestItem>) {
      stats.seen++;
      const id = harvestDecisionId(it);
      if (seenIds.has(id)) continue;
      seenIds.add(id);
      if (have.decisions.has(id) && have.outcomes.has(id)) { stats.skipped_existing++; continue; }
      const ck = `${it.spec.primitive_id}\u0000${it.capGroup ?? ""}`;
      const n = newPerBank.get(ck) ?? 0;
      if (o.capPerBank && n >= o.capPerBank) { stats.capped++; continue; }
      newPerBank.set(ck, n + 1);
      queue.push({ it, id, state: redactState(it.state, stats.redactions) });
    }
  }
  if (o.dryRun) {
    for (const q of queue) bump(stats, q.it);
    return stats;
  }
  let next = 0, done = 0;
  const worker = async () => {
    while (next < queue.length) {
      const q = queue[next++];
      try {
        if (!have.decisions.has(q.id)) {
          await o.router.decide(requestFor(q.it, q.id), q.state, { decisionId: q.id, ts: q.it.ts });
          await new Promise((r) => queueMicrotask(() => r(null))); // logDecision appends in a microtask
        }
        ledger.recordOutcome({ decision_id: q.id, truth: q.it.truth, source: q.it.outcome_source, label_source: q.it.label_source, ts: q.it.ts });
        stats.replayed++;
        bump(stats, q.it);
      } catch (e) {
        stats.failed++;
        if (stats.errors.length < 10) stats.errors.push(`${q.it.spec.primitive_id}: ${(e as Error).message}`);
      }
      if (++done % 100 === 0) o.log?.(`harvest: ${done}/${queue.length}`);
    }
  };
  await Promise.all(Array.from({ length: Math.max(1, o.concurrency ?? 4) }, worker));
  return stats;
}

function bump(stats: HarvestRunStats, it: HarvestItem) {
  const b = (stats.per_bank[it.spec.primitive_id] ??= {});
  b[it.label_source] = (b[it.label_source] ?? 0) + 1;
}
