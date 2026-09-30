// Offline AAI registration (replay driver, no browser/network) for POST /aai/conformance/chatgpt-dots.
import type { AaiRegistration } from "../../../src/aai/registry.js";
import { AGENT_ID } from "../manifest.js";
import { ChatGPTDotsProvider, ReplayDotsDriver, type ReplayMode, type ReplayOptions } from "../index.js";

const mk = (mode: ReplayMode = "normal", extra: ReplayOptions = {}) => {
  const driver = new ReplayDotsDriver({ mode, ...extra });
  return { driver, p: new ChatGPTDotsProvider({ driver, pacing: false, pollMs: 1, replyTimeoutMs: 2000 }) };
};
export function createOfflineAaiRegistration(): { registration: AaiRegistration; close?: () => Promise<void> } {
  const { driver, p } = mk("normal", { confirmation: { policy: "ask_first", text: "Send email to sam@example.com" }, tasks: [{ title: "Compile vendor notes", state: "in_progress" }] });
  return { registration: { provider: p, fixtures: {
    agentId: `${AGENT_ID}:nova-dot`, approvalId: driver.confirmationId, settleMs: 10,
    faulty: { vendor_down: () => mk("down").p, rate_limited: () => mk("rate_limited").p, auth_revoked: () => mk("logged_out").p, account_banned: () => mk("blocked").p, ui_changed: () => mk("drift").p },
  } } };
}
