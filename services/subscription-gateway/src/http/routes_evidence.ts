// GET /v1/evidence/:id — failure evidence a browser lane saved (adapters/_shared/evidence.ts):
// the failed selector key, page URL, redacted DOM excerpt, and the screenshot path (`?file=screenshot`
// streams the PNG). Scoped tasks:read, like the rest of the attempt history. Installing the router
// also installs the DB sink the adapters write through and prunes screenshots older than 7 days.
import { createReadStream, existsSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { Router, type Request, type Response } from "express";
import {
  evidenceDir,
  pruneEvidence,
  setEvidenceSink,
  type EvidenceRow,
} from "../../adapters/_shared/evidence.js";
import type { Db } from "../store/db.js";
import { requireScope, type GatewayDeps } from "./server.js";

export function insertEvidence(db: Db, r: EvidenceRow): void {
  db.prepare(
    `INSERT INTO attempt_evidence (id, task_id, adapter_id, selector_key, url, dom_excerpt, screenshot_file, detail, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`
  ).run(r.id, r.task_id, r.adapter_id, r.selector_key, r.url, r.dom_excerpt, r.screenshot_file, r.detail, r.created_at);
}

export function getEvidence(db: Db, id: string): EvidenceRow | null {
  return (db.prepare("SELECT * FROM attempt_evidence WHERE id = ?").get(id) as EvidenceRow | undefined) ?? null;
}

/** Newest evidence for an adapter in the last day, for the lane board's Proof link. */
export function latestEvidenceId(db: Db, adapterId: string): string | null {
  const since = new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString();
  const row = db
    .prepare("SELECT id FROM attempt_evidence WHERE adapter_id = ? AND created_at >= ? ORDER BY created_at DESC LIMIT 1")
    .get(adapterId, since) as { id: string } | undefined;
  return row?.id ?? null;
}

/**
 * The screenshot file for a stored evidence row, or null when there is none, it was pruned, or the
 * stored name is not a plain file name. Only files directly under the evidence dir are served: the
 * name is reduced to its basename and the resolved path must stay inside the dir.
 */
export function screenshotPath(row: Pick<EvidenceRow, "screenshot_file">, dir = evidenceDir()): string | null {
  const name = row.screenshot_file;
  if (!name || name !== basename(name) || name === "." || name === "..") return null;
  const root = resolve(dir);
  const path = resolve(root, name);
  if (dirname(path) !== root) return null;
  return existsSync(path) ? path : null;
}

export function evidenceRouter(deps: GatewayDeps): Router {
  const router = Router();
  setEvidenceSink((row) => {
    try {
      insertEvidence(deps.db, row);
    } catch {}
  });
  pruneEvidence();

  const sendScreenshot = (row: EvidenceRow, res: Response) => {
    const path = screenshotPath(row);
    if (!path) {
      res.status(410).json({ error: "screenshot_expired", evidence_id: row.id });
      return;
    }
    res.setHeader("Content-Type", "image/png");
    res.setHeader("X-Content-Type-Options", "nosniff");
    res.setHeader("Cache-Control", "private, max-age=300");
    createReadStream(path).pipe(res);
  };

  router.get("/v1/evidence/:id/screenshot", requireScope("tasks:read"), (req: Request, res: Response) => {
    const row = getEvidence(deps.db, req.params.id);
    if (!row) {
      res.status(404).json({ error: "evidence_not_found", evidence_id: req.params.id });
      return;
    }
    sendScreenshot(row, res);
  });

  router.get("/v1/evidence/:id", requireScope("tasks:read"), (req: Request, res: Response) => {
    const row = getEvidence(deps.db, req.params.id);
    if (!row) {
      res.status(404).json({ error: "evidence_not_found", evidence_id: req.params.id });
      return;
    }
    if (req.query.file === "screenshot") {
      sendScreenshot(row, res);
      return;
    }
    const { screenshot_file, ...rest } = row;
    res.json({
      ...rest,
      screenshot: screenshot_file && existsSync(join(evidenceDir(), basename(screenshot_file))) ? `/v1/evidence/${row.id}/screenshot` : null,
    });
  });

  return router;
}
