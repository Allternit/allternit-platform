// Offline AAI registration (replay driver, no CDP/app/network) for POST /aai/conformance/claude-desktop.
import type { AaiRegistration } from "../../../src/aai/registry.js";
import { AGENT_ID } from "../manifest.js";
import { ClaudeDesktopProvider, ReplayClaudeDesktopDriver, type ReplayMode } from "../index.js";

const mk = (mode: ReplayMode = "normal", extra: ConstructorParameters<typeof ReplayClaudeDesktopDriver>[0] = {}) => {
  const driver = new ReplayClaudeDesktopDriver({ mode, ...extra });
  return { driver, p: new ClaudeDesktopProvider({ driver, pacing: false, pollMs: 1, replyTimeoutMs: 2000 }) };
};
export function createOfflineAaiRegistration(): { registration: AaiRegistration; close?: () => Promise<void> } {
  const { driver, p } = mk("normal", { approval: "Finder" });
  return { registration: { provider: p, fixtures: {
    agentId: AGENT_ID, approvalId: driver.approvalId, settleMs: 10,
    faulty: { vendor_down: () => mk("down").p, rate_limited: () => mk("rate_limited").p, auth_revoked: () => mk("logged_out").p, account_banned: () => mk("blocked").p, ui_changed: () => mk("drift").p },
  } } };
}
