// ChatGPT on a linked iPhone (route iphone-app-ictrl). Spec: ../_shared/phone/chatgpt.ts.
import { DesktopPhoneGateway, type PhoneGateway } from "../../src/phone/gateway.js";
import { PhoneChatProvider, type PhoneChatProviderOptions } from "../_shared/phone/chat-provider.js";
import { CHATGPT_IPHONE } from "../_shared/phone/chatgpt.js";

export const ADAPTER_ID = CHATGPT_IPHONE.adapterId;
export const AGENT_ID = CHATGPT_IPHONE.agentId;
export const SPEC = CHATGPT_IPHONE;
/** One action a few seconds apart at most, like a person; well under any app limit. */
export const PACING = { min_task_gap_s: 6, max_tasks_per_hour: 40 };

export function create(opts: Partial<PhoneChatProviderOptions> & { gateway?: PhoneGateway } = {}): PhoneChatProvider {
  return new PhoneChatProvider(CHATGPT_IPHONE, { gateway: opts.gateway ?? new DesktopPhoneGateway(), ...opts });
}
