// gizzi-code sessions (~/.local/share/gizzi-code/gizzi.db, opened read-only) -> labelled items.
//   ROUTE        backfill_observed: the turn's tool parts, labelled by the live routeLabel rule.
//   ROUTE_MODEL  backfill_observed: the person's next move (accept / retry / model switch), live rule.
// A turn = one user message plus the assistant messages that follow it in the session.
import { Database } from "bun:sqlite";
import { existsSync } from "node:fs";
import { genClassOf, routeLabel, routeModelLabel } from "../labels.ts";
import { BANKS, routeState } from "../banks.ts";
import type { HarvestItem, HarvestSource } from "../types.ts";

export interface GizziRow { session_id: string; message_id: string; time_created: number; msg: any; parts: any[] }
interface Turn { id: string; sid: string; ts: string; text: string; tools: string[]; requested: string; model: string | null; errored: boolean }

export function turnsFromRows(rows: GizziRow[]): Turn[] {
  const bySession = new Map<string, GizziRow[]>();
  for (const r of rows) { if (!bySession.has(r.session_id)) bySession.set(r.session_id, []); bySession.get(r.session_id)!.push(r); }
  const turns: Turn[] = [];
  for (const [sid, rs] of bySession) {
    rs.sort((a, b) => a.time_created - b.time_created);
    let cur: Turn | null = null;
    for (const r of rs) {
      if (r.msg?.role === "user") {
        const text = r.parts.filter((p) => p?.type === "text" && !p.synthetic).map((p) => String(p.text ?? "")).join("\n");
        if (!text.trim()) { cur = null; continue; }
        const m = r.msg.model ?? {};
        cur = { id: r.message_id, sid, ts: new Date(r.time_created).toISOString(), text, tools: [], requested: `${m.providerID ?? ""}/${m.modelID ?? ""}`, model: m.modelID ?? null, errored: false };
        turns.push(cur);
      } else if (r.msg?.role === "assistant" && cur) {
        if (r.msg.modelID) cur.model = r.msg.modelID;
        if (r.msg.error) cur.errored = true;
        for (const p of r.parts) if (p?.type === "tool" && typeof p.tool === "string") cur.tools.push(p.tool);
      }
    }
  }
  return turns;
}

export function itemsFromTurns(turns: Turn[]): HarvestItem[] {
  const out: HarvestItem[] = [];
  const bySid = new Map<string, Turn[]>();
  for (const t of turns) { if (!bySid.has(t.sid)) bySid.set(t.sid, []); bySid.get(t.sid)!.push(t); }
  for (const ts of bySid.values()) {
    ts.forEach((t, i) => {
      const state = routeState(t.text);
      out.push({ spec: BANKS.route, key: `${t.sid}:${t.id}`, source: "gizzi", ts: t.ts, state, truth: routeLabel(t.tools), label_source: "backfill_observed", outcome_source: "replay:gizzi.turn_tools" });
      const cls = genClassOf(t.model);
      if (!cls) return;
      const n = ts[i + 1];
      const lbl = routeModelLabel({ text: t.text, cls, errored: t.errored, requested: t.requested }, n ? { text: n.text, cls: genClassOf(n.model), requested: n.requested } : null);
      out.push({ spec: BANKS.route_model, key: `${t.sid}:${t.id}`, source: "gizzi", ts: t.ts, state, truth: lbl.truth, label_source: "backfill_observed", outcome_source: `replay:gizzi.${lbl.source}`, incumbent: cls });
    });
  }
  return out;
}

export function gizziSource(dbPath: string): HarvestSource {
  return {
    name: "gizzi",
    *items() {
      if (!existsSync(dbPath)) return;
      const db = new Database(dbPath, { readonly: true });
      try {
        const msgs = db.query("select id, session_id, time_created, data from message").all() as { id: string; session_id: string; time_created: number; data: string }[];
        const parts = new Map<string, any[]>();
        for (const p of db.query("select message_id, data from part order by time_created").all() as { message_id: string; data: string }[]) {
          try { const v = JSON.parse(p.data); if (!parts.has(p.message_id)) parts.set(p.message_id, []); parts.get(p.message_id)!.push(v); } catch { /* skip */ }
        }
        const rows: GizziRow[] = [];
        for (const m of msgs) { try { rows.push({ session_id: m.session_id, message_id: m.id, time_created: m.time_created, msg: JSON.parse(m.data), parts: parts.get(m.id) ?? [] }); } catch { /* skip */ } }
        yield* itemsFromTurns(turnsFromRows(rows));
      } finally { db.close(); }
    },
  };
}
