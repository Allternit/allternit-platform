// Offline AAI registration (replayed phone screens, no Desktop/phone/vendor) for POST /aai/conformance/chatgpt-android-app.
import type { AaiRegistration } from "../../../src/aai/registry.js";
import { ReplayPhoneGateway, type ReplayMode } from "../../_shared/phone/replay.js";
import { PhoneChatProvider } from "../../_shared/phone/chat-provider.js";
import { CHATGPT_ANDROID } from "../../_shared/phone/chatgpt.js";

const fast = { pollMs: 1, settleMs: 0, replyTimeoutMs: 2000, sleep: () => Promise.resolve() };
const mk = (mode: ReplayMode = "normal") => new PhoneChatProvider(CHATGPT_ANDROID, { gateway: new ReplayPhoneGateway(CHATGPT_ANDROID, mode), ...fast });

export function createOfflineAaiRegistration(): { registration: AaiRegistration } {
  return {
    registration: {
      provider: mk(),
      fixtures: {
        agentId: CHATGPT_ANDROID.agentId,
        settleMs: 10,
        faulty: { vendor_down: () => mk("down"), rate_limited: () => mk("rate_limited"), auth_revoked: () => mk("logged_out"), account_banned: () => mk("verification"), ui_changed: () => mk("drift") },
      },
    },
  };
}
