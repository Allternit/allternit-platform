// ChatGPT.app (native macOS app, bundle id com.openai.chat) AX selector pack "chatgpt-app-ax-v1" for the chatgpt-dots adapter.
// Everything is UNVERIFIED: written from how a native AppKit/SwiftUI chat app usually exposes accessible names, and from the
// dots UI as described in the launch memo (which the DOM pack already treats as inferred). First consented live session must
// snapshot the real tree and correct these; unresolved critical keys read as ADAPTER_DRIFT.
import { AX_SELECTOR_FORMAT, type AxKeySpec, type AxSelector, type AxSelectorPack } from "../../_shared/ax/index.js";

export const CHATGPT_APP_BUNDLE_ID = "com.openai.chat";
export const PACK_VERSION = "chatgpt-app-ax-v1";
const s = (role: string, extra: Partial<AxSelector> = {}): AxSelector => ({ v: AX_SELECTOR_FORMAT, role, ...extra });
const key = (critical: boolean, ...alternatives: AxSelector[]): AxKeySpec => ({ critical, confidence: "unverified", alternatives });

export const CHATGPT_APP_AX_PACK: AxSelectorPack = {
  format: AX_SELECTOR_FORMAT,
  packVersion: PACK_VERSION,
  bundleId: CHATGPT_APP_BUNDLE_ID,
  keys: {
    composer: key(true, s("AXTextArea", { label: "ask chatgpt|message chatgpt|ask anything" }), s("AXTextField", { label: "ask chatgpt|message chatgpt|ask anything" })),
    userTurn: key(false, s("AXGroup", { label: "^(you said|your message)" })),
    assistantTurn: key(false, s("AXGroup", { label: "^(chatgpt said|chatgpt response|assistant message)" })),
    alert: key(false, s("AXGroup", { subrole: "AXApplicationAlert" }), s("AXGroup", { label: "^(alert|notice|banner)" })),
    dotRow: key(false, s("AXButton", { path: [{ role: "AXList", label: "^dots$" }] })),
    dotHeader: key(false, s("AXGroup", { label: "^dot header" })),
    confirmation: key(false, s("AXGroup", { label: "^(ask first|hand off)" })),
    tasksPanel: key(false, s("AXGroup", { label: "^tasks$" })),
    activity: key(false, s("AXGroup", { label: "^activity" })),
  },
};
