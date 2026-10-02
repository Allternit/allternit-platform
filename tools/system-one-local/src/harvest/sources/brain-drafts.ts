// Allternit Brain draft accept/reject (Allternit Brain/.incoming/{applied,rejected}/) ->
// bank.lesson_worthiness items. ONLY CommRails lessons-triage drafts carry the
// x_commrails candidate block (candidate_id, dag_id, node_id, verdict); every
// other producer (watch-brain.js, session dumps) writes a different document
// shape that is not a lesson_worthiness state, so those files are skipped.
// Labels mirror triage.rs report_applied_outcomes / report_rejected_outcomes:
//   applied  -> reusable_pattern=true, supported_by_events=true (observed human)
//   rejected -> false for the question(s) x_rejection.why names (none when "other")
// task_success gets no label from a review: approval says nothing about whether
// the original task succeeded (APPROVAL_LABELLED).
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { BANKS, lessonCandidateState } from "../banks.ts";
import type { HarvestItem, HarvestSource } from "../types.ts";

/** triage.rs question ids (kept in sync: commrails/src/lessons/triage.rs). */
export const Q_TASK_SUCCESS = "task_success";
export const Q_REUSABLE = "reusable_pattern";
export const Q_SUPPORTED = "supported_by_events";

/** Rebuild the triage candidate JSON from a draft (draft_markdown's evidence block). */
export function candidateFromDraft(d: Record<string, any>): Record<string, unknown> {
  const x = d.x_commrails ?? {};
  const md: string = d.updates?.[0]?.content ?? "";
  const num = (re: RegExp) => { const m = re.exec(md); return m ? Number(m[1]) : 0; };
  const list = (re: RegExp) => { const m = re.exec(md); return m ? m[1].split(",").map((s) => s.trim()).filter(Boolean) : []; };
  const excerpt = /### Node output excerpt[^\n]*\n+```+\w*\n([\s\S]*?)\n```+/.exec(md);
  return {
    candidate_id: x.candidate_id, kind: "process_learning", dag_id: x.dag_id, node_id: x.node_id, wih_id: x.wih_id,
    node_title: "",
    final_status: /final status:\s*([^·\n]+)/.exec(md)?.[1]?.trim() ?? null,
    attempts: num(/attempts:\s*(\d+)/), failed_attempts: num(/\(failed:\s*(\d+)\)/),
    evidence_refs: list(/evidence:\s*([^\n]+)/), receipt_ids: list(/receipts:\s*([^\n]+)/),
    ...(excerpt ? { output_excerpt: excerpt[1] } : {}),
  };
}

function* drafts(dir: string, sub: string): Generator<{ path: string; file: string; d: Record<string, any> }> {
  const p = join(dir, ".incoming", sub);
  let files: string[];
  try { files = readdirSync(p).filter((f) => f.endsWith(".json")).sort(); } catch { return; }
  for (const f of files) {
    let d: any;
    try { d = JSON.parse(readFileSync(join(p, f), "utf8")); } catch { continue; }
    yield { path: join(sub, f), file: f, d };
  }
}

const iso = (ms: number) => new Date(ms).toISOString();

export function brainDraftsSource(brainRoot: string): HarvestSource {
  return {
    name: "brain-drafts",
    *items() {
      const seen = new Set<string>();
      for (const { path, file, d } of [...drafts(brainRoot, "applied"), ...drafts(brainRoot, "rejected")]) {
        const x = d.x_commrails;
        if (!x?.candidate_id) continue; // not a lessons-triage draft: no lesson_worthiness state
        const cid = String(x.candidate_id);
        if (seen.has(cid)) continue; // reviewed once per candidate
        seen.add(cid);
        let ts = "2026-01-01T00:00:00Z";
        try { ts = iso(statSync(join(brainRoot, ".incoming", path)).mtimeMs); } catch { /* keep default */ }
        const state = lessonCandidateState(candidateFromDraft(d));
        const base = { source: "brain-drafts", ts, state, label_source: "observed" as const };
        if (path.startsWith("applied/")) {
          yield { ...base, spec: BANKS.lesson_reusable, key: `brain:${cid}:reusable`, truth: "true", outcome_source: "human:brain.draft_applied" };
          yield { ...base, spec: BANKS.lesson_supported, key: `brain:${cid}:supported`, truth: "true", outcome_source: "human:brain.draft_applied" };
        } else {
          const why = d.x_rejection?.why ?? "";
          if (why === "not-reusable" || why === "both") yield { ...base, spec: BANKS.lesson_reusable, key: `brain:${cid}:reusable`, truth: "false", outcome_source: "human:brain.draft_rejected" };
          if (why === "unsupported" || why === "both") yield { ...base, spec: BANKS.lesson_supported, key: `brain:${cid}:supported`, truth: "false", outcome_source: "human:brain.draft_rejected" };
        }
      }
    },
  };
}
