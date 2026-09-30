#!/usr/bin/env bun
// Summarise the PreToolUse guard's dry-run log.
//   bun scripts/dryrun-summary.ts [--dir ~/.allternit/system-one/dryrun] [--json]
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { BASE_DIR } from "../src/log.ts";
import type { GuardRecord } from "../src/hook/guard.ts";

export interface Summary {
  files: number;
  calls: number;
  per_day: Record<string, number>;
  mean_tokens: { actual: number | null; estimated: number | null };
  pct_settled_by_hard_rules: number;
  hard_rule_breakdown: Record<string, number>;
  pct_escalated: number; // would_escalate (log mode) or emitted (advise), over all calls
  pct_escalated_of_consulted: number;
  server: Record<string, number>;
  redaction_flags: { calls_with_secret: number; calls_with_email: number; calls_with_client: number; calls_with_possible_name: number };
  methods: Record<string, number>;
  mean_latency_ms: number | null;
}

const pct = (a: number, b: number) => (b ? Math.round((1000 * a) / b) / 10 : 0);
const mean = (xs: number[]) => (xs.length ? Math.round(xs.reduce((a, b) => a + b, 0) / xs.length) : null);

export function summarize(records: GuardRecord[], files = 0): Summary {
  const per_day: Record<string, number> = {};
  const server: Record<string, number> = {};
  const hard: Record<string, number> = {};
  const methods: Record<string, number> = {};
  const actual: number[] = [], est: number[] = [], lat: number[] = [];
  let settled = 0, escalated = 0, consulted = 0;
  const flags = { calls_with_secret: 0, calls_with_email: 0, calls_with_client: 0, calls_with_possible_name: 0 };
  for (const r of records) {
    const day = r.ts.slice(0, 10);
    per_day[day] = (per_day[day] ?? 0) + 1;
    server[r.server] = (server[r.server] ?? 0) + 1;
    if (r.hard_rule) {
      settled++;
      hard[`${r.hard_rule.verdict}:${r.hard_rule.rule}`] = (hard[`${r.hard_rule.verdict}:${r.hard_rule.rule}`] ?? 0) + 1;
    }
    if (r.server === "ok") consulted++;
    if (r.would_escalate) escalated++;
    if (r.usage) actual.push(r.usage.input_tokens + r.usage.output_tokens);
    if (r.pack) est.push(r.pack.token_estimate);
    lat.push(r.latency_ms);
    if (r.redactions) {
      if (r.redactions.secret) flags.calls_with_secret++;
      if (r.redactions.email) flags.calls_with_email++;
      if (r.redactions.client) flags.calls_with_client++;
      if (r.redactions.possible_name) flags.calls_with_possible_name++;
    }
    for (const m of Object.values(r.methods ?? {})) methods[m] = (methods[m] ?? 0) + 1;
  }
  return {
    files,
    calls: records.length,
    per_day,
    mean_tokens: { actual: mean(actual), estimated: mean(est) },
    pct_settled_by_hard_rules: pct(settled, records.length),
    hard_rule_breakdown: hard,
    pct_escalated: pct(escalated, records.length),
    pct_escalated_of_consulted: pct(escalated, consulted),
    server,
    redaction_flags: flags,
    methods,
    mean_latency_ms: mean(lat),
  };
}

export function readDir(dir: string): { records: GuardRecord[]; files: number } {
  if (!existsSync(dir)) return { records: [], files: 0 };
  const files = readdirSync(dir).filter((f) => f.endsWith(".jsonl")).sort();
  const records: GuardRecord[] = [];
  for (const f of files) {
    for (const line of readFileSync(join(dir, f), "utf8").split("\n")) {
      if (!line.trim()) continue;
      try { records.push(JSON.parse(line)); } catch { /* skip corrupt line */ }
    }
  }
  return { records, files: files.length };
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const di = args.indexOf("--dir");
  const dir = di >= 0 ? args[di + 1] : (process.env.SYSTEM_ONE_DRYRUN_DIR ?? join(BASE_DIR, "dryrun"));
  const { records, files } = readDir(dir);
  const s = summarize(records, files);
  if (args.includes("--json")) {
    console.log(JSON.stringify(s, null, 2));
  } else {
    console.log(`dry-run log: ${dir} (${files} files, ${s.calls} calls)`);
    console.log("calls/day:");
    for (const [d, n] of Object.entries(s.per_day)) console.log(`  ${d}  ${n}`);
    console.log(`mean tokens/call: actual ${s.mean_tokens.actual ?? "n/a"}, estimated ${s.mean_tokens.estimated ?? "n/a"}`);
    console.log(`settled by hard rules: ${s.pct_settled_by_hard_rules}%  ${JSON.stringify(s.hard_rule_breakdown)}`);
    console.log(`escalated (allow→ask): ${s.pct_escalated}% of all calls, ${s.pct_escalated_of_consulted}% of server-consulted calls`);
    console.log(`server: ${JSON.stringify(s.server)}   methods: ${JSON.stringify(s.methods)}`);
    console.log(`redaction flags (calls): secrets ${s.redaction_flags.calls_with_secret}, emails ${s.redaction_flags.calls_with_email}, client-looking ${s.redaction_flags.calls_with_client}, possible person names ${s.redaction_flags.calls_with_possible_name}`);
    console.log(`mean hook latency: ${s.mean_latency_ms ?? "n/a"} ms`);
  }
}
