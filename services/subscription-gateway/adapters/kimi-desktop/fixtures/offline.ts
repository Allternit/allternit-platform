// Offline AAI registration (scripted AX app, no bridge/app/network) for POST /aai/conformance/kimi-desktop.
import type { AaiRegistration } from "../../../src/aai/registry.js";
import { AxChatAppProvider, scriptedChatApp, type ChatScriptMode } from "../../_shared/ax/index.js";
import { KIMI_DESKTOP_SKIN, KIMI_DESKTOP_SPEC } from "../index.js";

const mk = (mode: ChatScriptMode = "normal") =>
  new AxChatAppProvider(KIMI_DESKTOP_SPEC, { ax: scriptedChatApp(KIMI_DESKTOP_SKIN, { mode }).ax, consented: true, pacing: false, pollMs: 1, replyTimeoutMs: 2000 });
export function createOfflineAaiRegistration(): { registration: AaiRegistration; close?: () => Promise<void> } {
  return { registration: { provider: mk(), fixtures: {
    agentId: KIMI_DESKTOP_SPEC.agentId, settleMs: 10,
    faulty: { vendor_down: () => mk("down"), rate_limited: () => mk("rate_limited"), auth_revoked: () => mk("logged_out"), account_banned: () => mk("blocked"), ui_changed: () => mk("drift") },
  } } };
}
