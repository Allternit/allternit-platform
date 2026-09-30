// Offline AAI registration (replay driver, no CDP/app/network) for POST /aai/conformance/grok-bot.
import type { AaiRegistration } from "../../../src/aai/registry.js";
import { AGENT_ID } from "../manifest.js";
import { GrokBotProvider, ReplayGrokDriver, type ReplayMode } from "../index.js";

const mk = (mode: ReplayMode = "normal", extra: ConstructorParameters<typeof ReplayGrokDriver>[0] = {}) => {
  const driver = new ReplayGrokDriver({ mode, ...extra });
  return { driver, p: new GrokBotProvider({ driver, pacing: false, pollMs: 1, replyTimeoutMs: 2000 }) };
};
export function createOfflineAaiRegistration(): { registration: AaiRegistration; close?: () => Promise<void> } {
  const { driver, p } = mk("normal", { approval: "Send email to sam@example.com" });
  return { registration: { provider: p, fixtures: {
    agentId: AGENT_ID, approvalId: driver.approvalId, settleMs: 10,
    faulty: { vendor_down: () => mk("down").p, rate_limited: () => mk("rate_limited").p, auth_revoked: () => mk("logged_out").p, account_banned: () => mk("blocked").p, ui_changed: () => mk("drift").p },
  } } };
}
