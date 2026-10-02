// agent-ledger session summaries (agent-ledger/summaries/*.md) -> judge.first_pass.node items.
// The judge's node question is "did the worker accomplish the task" (backends.rs
// node_verdict). A summary is the worker's own untrusted report; git history is
// the deterministic verifier: the session's PR merged into origin/main (merge
// commit "Merge pull request #N" or squash subject "(#N)") -> task_complete=true
// (source verifier:git.merged); the PR closed unmerged -> false (verifier:gh.closed).
// Summaries whose PR cannot be verified either way get NO label: an unverifiable
// session is not evidence of failure (squash merges and cross-repo PRs hide here).
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { BANKS, judgeNodeState } from "../banks.ts";
import type { HarvestItem, HarvestSource } from "../types.ts";

/** PR numbers (strings) verified against the shared checkout's origin/main. */
export interface PrVerdicts { merged: Set<string>; closed: Set<string> }

const PR_REF = /(?:\bPRs?\s*|pull request\s*)#(\d{1,5})/i;
const DATE_PREFIX = /^(\d{4}-\d{2}-\d{2})-/;

/** The session's own PR: the first PR reference in the summary (lifecycle records it up front). */
export function firstPrRef(text: string): string | null {
  const m = PR_REF.exec(text);
  return m ? m[1]! : null;
}

export interface LedgerSummary { file: string; ts: string; title: string; body: string; pr: string | null }

export function parseSummary(file: string, text: string): LedgerSummary {
  const head = text.slice(0, 6000);
  const title = (head.match(/^#\s+(.+)$/m)?.[1] ?? file.replace(/\.md$/, "")).trim();
  const dm = DATE_PREFIX.exec(file);
  const pr = firstPrRef(head);
  // The "verification" section, when the summary carries one, is the task's acceptance.
  const vm = head.match(/^##\s+verification[^#]*/im);
  const body = head.slice(0, 2400);
  return { file, ts: dm ? `${dm[1]}T12:00:00Z` : "2026-01-01T00:00:00Z", title, body: vm ? `${body}\n\n${vm[0].slice(0, 800)}` : body, pr };
}

export function agentLedgerSource(dir: string, verdicts: PrVerdicts): HarvestSource {
  return {
    name: "agent-ledger",
    *items() {
      let files: string[];
      try { files = readdirSync(dir).filter((f) => f.endsWith(".md")).sort(); } catch { return; }
      for (const f of files) {
        let text: string;
        try { text = readFileSync(join(dir, f), "utf8"); } catch { continue; }
        const s = parseSummary(f, text);
        if (!s.pr) continue; // no PR reference: nothing a human can verify
        const truth = verdicts.merged.has(s.pr) ? "true" : verdicts.closed.has(s.pr) ? "false" : null;
        if (truth === null) continue; // unverifiable: no invented label
        const merged = truth === "true";
        yield {
          spec: BANKS.judge_node, key: `agent-ledger:${s.file}`, source: "agent-ledger", ts: s.ts,
          state: judgeNodeState({ title: s.title, description: `agent-ledger summary ${s.file}`, output: s.body, evidenceRefs: [s.file, `PR #${s.pr}`] }),
          truth,
          label_source: "backfill_observed",
          outcome_source: merged ? "verifier:git.merged" : "verifier:gh.closed_unmerged",
        };
      }
    },
  };
}
