// Phone Link pairing handover, receiving side (Eoj, 2026-10-17). The person's
// Mac gives a linked phone's pairing to this Sessions computer so its Desktop
// serves the phone with the Mac off. allternit-api reaches this route only on
// the caller's own bound Sessions computer (subscription_routes.rs
// /subscriptions/sessions-computer/phone-handover); the payload is sealed to
// this computer's Desktop key end to end, so the gateway only relays it to
// Desktop's loopback phone gateway (surfaces/allternit-desktop/src/main
// phone-gateway.ts, phone-link-handover.ts). Never a re-pair: the phone helpers
// refuse pairing over the mesh.
//
//   GET  /v1/phone/handover/key            → Desktop's HandoverKey
//   POST /v1/phone/handover {sealed}       → { ok, deviceId, platform, name }
import { Router, type Request, type Response } from "express";
import { requireScope, type GatewayDeps } from "./server.js";
import { DesktopPhoneGateway } from "../phone/gateway.js";

export interface PhoneHandoverRelay {
  request(method: "GET" | "POST", path: string, body?: unknown): Promise<{ status: number; body: unknown }>;
}

function send(res: Response, r: { status: number; body: unknown }): void {
  // 0 = Desktop isn't running here: the Sessions computer can't take a phone now.
  res.status(r.status === 0 ? 503 : r.status).json(r.body ?? { ok: false, error: "desktop_unavailable" });
}

export function phoneHandoverRouter(deps: GatewayDeps): Router {
  const router = Router();
  const relay = (): PhoneHandoverRelay =>
    deps.phoneHandover ?? new DesktopPhoneGateway({ file: process.env.SUBS_GATEWAY_PHONE_GATEWAY_FILE });

  router.get("/v1/phone/handover/key", requireScope("accounts:manage"), async (_req: Request, res: Response) => {
    send(res, await relay().request("GET", "/phone/handover/key"));
  });

  router.post("/v1/phone/handover", requireScope("accounts:manage"), async (req: Request, res: Response) => {
    const sealed = (req.body as { sealed?: unknown } | undefined)?.sealed;
    if (!sealed || typeof sealed !== "object") {
      res.status(400).json({ ok: false, error: "sealed_required" });
      return;
    }
    send(res, await relay().request("POST", "/phone/handover", { sealed }));
  });

  return router;
}
