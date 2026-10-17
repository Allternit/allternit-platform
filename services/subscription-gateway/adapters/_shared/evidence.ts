// ui_bridge failure evidence: when a browser step fails (a selector matches nothing, the composer
// drifts) the adapter saves a screenshot, the failed selector key, the page URL and a redacted DOM
// excerpt. The screenshot goes under ~/.allternit/subscriptions/evidence/ (kept 7 days, pruned when
// the gateway starts); the rest goes to the `attempt_evidence` row (migration 0011) through the sink
// the HTTP layer installs. The returned id is TaskError.evidence_ref; GET /v1/evidence/:id serves it.
import { randomUUID } from "node:crypto";
import { mkdirSync, readdirSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { Page } from "playwright";
import { redactExcerpt, redactText } from "../../src/security/redact.js";

export const EVIDENCE_TTL_MS = 7 * 24 * 60 * 60 * 1000;
const DOM_MAX = 2000;

export interface EvidenceRow {
  id: string;
  task_id: string | null;
  adapter_id: string | null;
  selector_key: string | null;
  url: string | null;
  dom_excerpt: string | null;
  screenshot_file: string | null;
  detail: string | null;
  created_at: string;
}

export type EvidenceSink = (row: EvidenceRow) => void;
let sink: EvidenceSink | null = null;

/** The HTTP layer installs the DB writer; without one, evidence is dropped (no ref is set). */
export function setEvidenceSink(next: EvidenceSink | null): void {
  sink = next;
}

export function evidenceDir(): string {
  return process.env.ALLTERNIT_EVIDENCE_DIR || join(homedir(), ".allternit", "subscriptions", "evidence");
}

/** Delete screenshots older than 7 days. Returns how many were removed. */
export function pruneEvidence(dir = evidenceDir(), now = Date.now()): number {
  let removed = 0;
  try {
    for (const f of readdirSync(dir)) {
      const p = join(dir, f);
      try {
        if (now - statSync(p).mtimeMs > EVIDENCE_TTL_MS) {
          unlinkSync(p);
          removed++;
        }
      } catch {}
    }
  } catch {}
  return removed;
}

export interface EvidenceInput {
  taskId?: string | null;
  adapterId?: string | null;
  selectorKey?: string | null;
  detail?: string | null;
}

/** Best effort and never throws: a failed capture must not hide the failure it documents. */
export async function captureEvidence(page: Page, input: EvidenceInput): Promise<string | null> {
  if (!sink) return null;
  try {
    const id = randomUUID();
    const dir = evidenceDir();
    mkdirSync(dir, { recursive: true });
    let screenshot_file: string | null = null;
    try {
      writeFileSync(join(dir, `${id}.png`), await page.screenshot({ type: "png", timeout: 5000 }));
      screenshot_file = `${id}.png`;
    } catch {}
    let dom: string | null = null;
    try {
      dom = redactExcerpt(await page.evaluate(() => document.body?.innerHTML ?? ""), DOM_MAX);
    } catch {}
    let url: string | null = null;
    try {
      const u = new URL(page.url());
      url = `${u.origin}${u.pathname}`; // no query or fragment: they can carry tokens
    } catch {}
    sink({
      id,
      task_id: input.taskId ?? null,
      adapter_id: input.adapterId ?? null,
      selector_key: input.selectorKey ?? null,
      url,
      dom_excerpt: dom,
      screenshot_file,
      detail: input.detail ? redactText(input.detail).slice(0, 300) : null,
      created_at: new Date().toISOString(),
    });
    return id;
  } catch {
    return null;
  }
}
