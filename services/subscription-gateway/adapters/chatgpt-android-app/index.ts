// ChatGPT on a linked Android phone (route android-app). Spec: ../_shared/phone/chatgpt.ts.
import { DesktopPhoneGateway, type PhoneGateway } from "../../src/phone/gateway.js";
import { PhoneChatProvider, type PhoneChatProviderOptions } from "../_shared/phone/chat-provider.js";
import { CHATGPT_ANDROID } from "../_shared/phone/chatgpt.js";

export const ADAPTER_ID = CHATGPT_ANDROID.adapterId;
export const AGENT_ID = CHATGPT_ANDROID.agentId;
export const SPEC = CHATGPT_ANDROID;
/** One action a few seconds apart at most, like a person; well under any app limit. */
export const PACING = { min_task_gap_s: 6, max_tasks_per_hour: 40 };

export function create(opts: Partial<PhoneChatProviderOptions> & { gateway?: PhoneGateway } = {}): PhoneChatProvider {
  return new PhoneChatProvider(CHATGPT_ANDROID, { gateway: opts.gateway ?? new DesktopPhoneGateway(), ...opts });
}
