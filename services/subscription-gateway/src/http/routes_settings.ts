// GET/PUT /v1/settings/browser-lanes — whether browser sign-ins may run on
// this computer (local_desktop.ts). On a Sessions computer they always may and
// PUT answers 409. Turning them off closes every running browser lane.
import { Router, type Request, type Response } from "express";
import { z } from "zod";
import { listAccounts } from "../store/queries.js";
import { readBrowserLanes, writeBrowserLanes } from "../local_desktop.js";
import { detectGoogleChrome } from "../worker/hidden_chrome.js";
import { requireScope, type GatewayDeps } from "./server.js";

const putSchema = z.object({ enabled: z.boolean() }).strict();

export function settingsRouter(deps: GatewayDeps): Router {
  const router = Router();
  const view = () => {
    const setting = readBrowserLanes(deps.config.stateDir);
    // Only the person's own computer runs the hidden Chrome; say whether it is installed.
    return { ...setting, chrome_installed: setting.local_desktop ? detectGoogleChrome() !== null : null };
  };

  router.get("/v1/settings/browser-lanes", requireScope("accounts:manage"), (_req: Request, res: Response) => {
    res.json(view());
  });

  router.put("/v1/settings/browser-lanes", requireScope("accounts:manage"), async (req: Request, res: Response) => {
    const parsed = putSchema.safeParse(req.body);
    if (!parsed.success) {
      res.status(400).json({ error: "invalid_body", detail: parsed.error.issues.map((i) => i.message).join("; ") });
      return;
    }
    if (!readBrowserLanes(deps.config.stateDir).local_desktop) {
      res.status(409).json({ error: "not_local_desktop", detail: "Browser sign-ins always run on a Sessions computer." });
      return;
    }
    writeBrowserLanes(deps.config.stateDir, parsed.data.enabled);
    if (!parsed.data.enabled && deps.pool) {
      for (const a of listAccounts(deps.db)) {
        await deps.pool.deactivate({ provider: a.provider, account_id: a.account_id }).catch(() => {});
      }
    }
    res.json(view());
  });

  return router;
}
