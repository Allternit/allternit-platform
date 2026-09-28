// GET /v1/artifacts + GET /v1/artifacts/:id — metadata rows.
// GET /v1/artifacts/:id/download — the stored bytes, always as an attachment
// that cannot render inline (D6: the media-router's ChatGPT image lane pulls
// its PNG through this). Sandboxed HTML preview lands with the preview work.
import { createReadStream } from "node:fs";
import { isAbsolute, join, normalize, sep } from "node:path";
import { Router, type Request, type Response } from "express";
import { getArtifact, listArtifacts } from "../store/queries.js";
import { requireScope, type GatewayDeps } from "./server.js";

export function artifactsRouter(deps: GatewayDeps): Router {
  const router = Router();

  router.get("/v1/artifacts", requireScope("artifacts:read"), (_req: Request, res: Response) => {
    res.json(listArtifacts(deps.db));
  });

  router.get("/v1/artifacts/:id", requireScope("artifacts:read"), (req: Request, res: Response) => {
    const artifact = getArtifact(deps.db, req.params.id);
    if (!artifact) {
      res.status(404).json({ error: "artifact_not_found", artifact_id: req.params.id });
      return;
    }
    res.json(artifact);
  });

  router.get("/v1/artifacts/:id/download", requireScope("artifacts:read"), (req: Request, res: Response) => {
    const artifact = getArtifact(deps.db, req.params.id);
    if (!artifact) {
      res.status(404).json({ error: "artifact_not_found", artifact_id: req.params.id });
      return;
    }
    const rel = artifact.storage.local_path;
    if (artifact.storage.retrieval_state !== "local" || !rel) {
      res.status(409).json({
        error: "artifact_not_local",
        artifact_id: artifact.artifact_id,
        retrieval_state: artifact.storage.retrieval_state,
      });
      return;
    }
    // local_path is relative to the artifact root; refuse anything that
    // would resolve outside it.
    const root = normalize(deps.config.artifactsDir);
    const file = normalize(join(root, rel));
    if (isAbsolute(rel) || !file.startsWith(root + sep)) {
      res.status(409).json({ error: "artifact_path_invalid", artifact_id: artifact.artifact_id });
      return;
    }
    const name = `${artifact.artifact_id}${artifact.format ? `.${artifact.format}` : ""}`;
    res.set({
      "content-type": artifact.mime_type ?? "application/octet-stream",
      "content-disposition": `attachment; filename="${name}"`,
      "x-content-type-options": "nosniff",
      "content-security-policy": "default-src 'none'; sandbox",
      "cache-control": "no-store",
      ...(artifact.storage.sha256 ? { "x-artifact-sha256": artifact.storage.sha256 } : {}),
    });
    createReadStream(file)
      .on("error", () => {
        if (!res.headersSent) res.status(410).json({ error: "artifact_file_missing", artifact_id: artifact.artifact_id });
        else res.destroy();
      })
      .pipe(res);
  });

  return router;
}
