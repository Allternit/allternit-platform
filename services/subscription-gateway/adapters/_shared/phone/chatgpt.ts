// ChatGPT on a Phone-Linked phone. Labels recorded on real devices 2026-10-17:
// iPhone 15 / iOS 26.6.2 (com.openai.chat) and moto g play 2024 / Android 14
// (com.openai.chatgpt). Shared by adapters/chatgpt-android-app and chatgpt-iphone-app.
import { agentCapabilityManifestSchema } from "@allternit/subscription-fabric-contracts";
import type { PhoneChatAppSpec } from "./chat-provider.js";

const capabilities = (adapterId: string) =>
  agentCapabilityManifestSchema.parse({
    vendor: "chatgpt",
    adapterId,
    lane: "ui_bridge",
    guarantee: "best_effort",
    // One conversation on the phone screen at a time; ChatGPT keeps account-level memory.
    context: { supported: true, resume: false, parallel: false, maxParallel: 1, isolation: "shared" },
    messaging: { send: true, stream: true, steer: false, interrupt: false, cancel: true },
    memory: { read: false, write: false, snapshot: false, opaque: true },
    tools: { tools: false, mcp: false, plugins: false, connectors: false },
    tasks: { list: false, schedule: false, cancel: false, background: false },
    approvals: { read: false, respond: false, exact: false },
    computer: { view: false, control: false, takeover: false },
    artifacts: { read: false, write: false, export: false },
    events: { native: false, polling: true, transcriptDerived: true, replay: true },
    runtime: { alwaysOn: false, localRequired: true, cloud: false },
  });

const RATE_LIMIT = /(you'?ve reached|reached (the|your)).*limit|usage limit|too many (requests|messages)/i;
const VERIFICATION = /verify (you('re| are) )?(a )?human|unusual activity|suspicious activity/i;

export const CHATGPT_ANDROID: PhoneChatAppSpec = {
  vendor: "chatgpt",
  adapterId: "chatgpt-android-app",
  agentId: "chatgpt-android-app",
  appName: "ChatGPT",
  route: "android-app",
  platform: "android",
  appId: "com.openai.chatgpt",
  composer: "Ask ChatGPT",
  send: /^Send Message$/,
  stop: /^Stop/i,
  signedOut: /^Log in$/,
  rateLimit: RATE_LIMIT,
  verification: VERIFICATION,
  chrome: /^(.|What can I help with\?|What’s on your mind\?|Ask ChatGPT|Reply to ChatGPT|Menu|Log in|Attachment|Dictation|Send Message|Copy|Read aloud|Share|More actions|Ad)$/,
  capabilities: capabilities("chatgpt-android-app"),
};

export const CHATGPT_IPHONE: PhoneChatAppSpec = {
  vendor: "chatgpt",
  adapterId: "chatgpt-iphone-app",
  agentId: "chatgpt-iphone-app",
  appName: "ChatGPT",
  route: "iphone-app-ictrl",
  platform: "ios",
  appId: "com.openai.chat",
  composer: "New Message",
  send: /^(Send|MessageInputPrimaryButtonContainerPart\.PrimaryButton)$/,
  stop: /^Stop/i,
  signedOut: /^(Log in|Sign up)$/,
  rateLimit: RATE_LIMIT,
  verification: VERIFICATION,
  chrome: /^(.|ChatGPT|Ask ChatGPT|New Message|Menu|Chat|Work|Create an image or sticker|Write or edit|Search the web|Switch to Voice|Typing Predictions|Add|Dictation|Intelligence|Send|Copy|Share|Read aloud|More)$/,
  capabilities: capabilities("chatgpt-iphone-app"),
};
