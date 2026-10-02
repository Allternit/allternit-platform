// Desktop allternit-api SQLite (~/Library/Application Support/@allternit/desktop/allternit/allternit.db,
// opened read-only) -> labelled items for the banks whose live callers read it.
//   memory.relation  <- memory_relationships rows: state = 'new message:\n{obs}\n\nexisting memory:\n{fact}'
//                       (memory_relations.rs shadow_turn), truth = the stored relation_type
//                       (origin 'user' rows are human edits -> observed; else backfill_observed).
//   ROUTE_MODEL      <- usage ledger rows (llm_usage_events / gizzi_code_usage_events): the model
//                       class that actually served the call, live genClassOf tiering (backfill).
// What this Mac's history actually contains is reported by the run stats: the memory
// write path never produced facts/edges here (memory_facts and memory_relationships are
// empty and observations carry no type), and the usage tables are near-empty, so these
// queries legitimately yield ~0 rows today. MEMORY_TYPE is not harvestable: its live
// caller sends operation LABEL, which this runtime does not serve.
import { Database } from "bun:sqlite";
import { existsSync } from "node:fs";
import { BANKS } from "../banks.ts";
import { genClassOf } from "../labels.ts";
import type { HarvestItem, HarvestSource } from "../types.ts";

export function desktopDbSource(dbPath: string): HarvestSource {
  return {
    name: "desktop-db",
    *items() {
      if (!existsSync(dbPath)) return;
      const db = new Database(dbPath, { readonly: true });
      try {
        // memory.relation: one row per typed edge with its source observation + target fact.
        const rels = db.query(`SELECT r.id, r.relation_type, r.origin, r.valid_from, o.content AS obs_content, f.fact AS fact_text
          FROM memory_relationships r
          LEFT JOIN memory_observations o ON o.id = r.source_entity_id
          LEFT JOIN memory_facts f ON f.id = r.target_entity_id`).all() as { id: string; relation_type: string | null; origin: string | null; valid_from: string | null; obs_content: string | null; fact_text: string | null }[];
        for (const r of rels) {
          if (!r.relation_type) continue;
          const user = r.origin === "user";
          yield {
            spec: BANKS.memory_relation, key: `desktop-db:rel:${r.id}`, source: "desktop-db",
            ts: r.valid_from ? new Date(r.valid_from.replace(" ", "T") + "Z").toISOString() : "2026-01-01T00:00:00Z",
            state: `new message:\n${(r.obs_content ?? "").slice(0, 2000)}\n\nexisting memory:\n${r.fact_text ?? ""}`,
            truth: r.relation_type,
            label_source: user ? "observed" : "backfill_observed",
            outcome_source: user ? "human:memory.user_edge" : "memory.incumbent",
          } satisfies HarvestItem;
        }
        // ROUTE_MODEL: the class of the model that actually served each usage-ledger call.
        const usage = db.query(`SELECT model_id, created_at FROM llm_usage_events WHERE model_id IS NOT NULL
          UNION ALL SELECT model_id, created_at FROM gizzi_code_usage_events WHERE model_id IS NOT NULL`).all() as { model_id: string; created_at: string }[];
        for (const [i, u] of usage.entries()) {
          const cls = genClassOf(u.model_id);
          if (!cls) continue; // unclassifiable provider-only ids get no label (live rule)
          yield {
            spec: BANKS.route_model, key: `desktop-db:usage:${i}:${u.model_id}`, source: "desktop-db",
            ts: /^\d{4}-\d{2}-\d{2} /.test(u.created_at) ? new Date(u.created_at.replace(" ", "T") + "Z").toISOString() : u.created_at,
            state: `usage ledger call served by ${u.model_id}`, truth: cls,
            label_source: "backfill_observed", outcome_source: "replay:usage.served_model", incumbent: cls,
          } satisfies HarvestItem;
        }
      } finally { db.close(); }
    },
  };
}
