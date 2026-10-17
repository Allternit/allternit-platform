// ChatGPT desktop app AX selector pack "chatgpt-desktop-ax-v1" (route desktop-app-ax).
// Two builds answer to /Applications/ChatGPT.app: the native SwiftUI app (bundle com.openai.chat) and, on this Mac
// (2026-10-17), the Electron build that ships as "ChatGPT" with bundle id com.openai.codex v26.908.70816 (app.asar,
// "Codex Framework.framework"). The provider attaches to whichever is running, native first.
// EVERY selector is UNVERIFIED: the app was not running during recon and was not launched to record a tree. Labels are
// the accessible names chatgpt.com gives its controls (aria-labels as Chromium exposes them) plus common native wording.
// The first consented live session must snapshot the real tree and correct these; an unresolved critical key reads as
// ADAPTER_DRIFT by design. Bump PACK_VERSION when the live UI drifts.
import { AX_SELECTOR_FORMAT, type AxKeySpec, type AxSelector, type AxSelectorPack, type ChatSkin } from "../_shared/ax/index.js";

export const CHATGPT_NATIVE_BUNDLE_ID = "com.openai.chat";
export const CHATGPT_ELECTRON_BUNDLE_ID = "com.openai.codex";
export const CHATGPT_BUNDLE_IDS = [CHATGPT_NATIVE_BUNDLE_ID, CHATGPT_ELECTRON_BUNDLE_ID];
export const PACK_VERSION = "chatgpt-desktop-ax-v1";
const s = (role: string, extra: Partial<AxSelector> = {}): AxSelector => ({ v: AX_SELECTOR_FORMAT, role, ...extra });
const key = (critical: boolean, ...alternatives: AxSelector[]): AxKeySpec => ({ critical, confidence: "unverified", alternatives });
const COMPOSER = "^(ask anything|message chatgpt|ask chatgpt|chat with chatgpt)";

export const CHATGPT_DESKTOP_AX_PACK: AxSelectorPack = {
  format: AX_SELECTOR_FORMAT,
  packVersion: PACK_VERSION,
  bundleId: CHATGPT_ELECTRON_BUNDLE_ID,
  keys: {
    composer: key(true, s("AXTextArea", { label: COMPOSER }), s("AXTextField", { label: COMPOSER })),
    send: key(true, s("AXButton", { label: "^(send prompt|send message|send)$" })),
    stop: key(false, s("AXButton", { label: "^(stop streaming|stop generating|stop)$" })),
    newChat: key(true, s("AXButton", { label: "^new chat$" }), s("AXLink", { label: "^new chat$" })),
    userTurn: key(false, s("AXGroup", { label: "^(you said|your message|user message)" })),
    assistantTurn: key(false, s("AXGroup", { label: "^(chatgpt said|chatgpt response|assistant message)" })),
    alert: key(false, s("AXGroup", { subrole: "AXApplicationAlert" }), s("AXGroup", { label: "^(alert|notice|banner)" })),
    signIn: key(false, s("AXButton", { label: "^(log in|sign in)$" })),
    modelPicker: key(false, s("AXPopUpButton", { label: "^model selector" }), s("AXButton", { label: "^model selector" })),
    modelOption: key(false, s("AXMenuItem", { path: [{ role: "AXMenu" }] })),
    threadRow: key(false, s("AXLink", { path: [{ role: "AXGroup", label: "^(chat history|chats|sidebar)" }] }), s("AXButton", { path: [{ role: "AXGroup", label: "^(chat history|chats)" }] })),
    attach: key(false, s("AXButton", { label: "^(add photos (and|&) files|attach files?|upload)" })),
  },
};

/** The fixture skin mirrors the pack's first alternatives (tests fail if the pack stops resolving its own skin). */
export const CHATGPT_DESKTOP_SKIN: ChatSkin = {
  appTitle: "ChatGPT", bundleId: CHATGPT_ELECTRON_BUNDLE_ID, webArea: true,
  composer: { role: "AXTextArea", description: "Ask anything" },
  send: "Send prompt", stop: "Stop streaming", newChat: "New chat",
  userTurn: "You said", assistantTurn: "ChatGPT said", signIn: "Log in",
  modelPicker: { role: "AXPopUpButton", description: "Model selector" }, modelMenu: "Models",
  sidebar: "Chat history", threadRole: "AXLink", attach: "Add photos & files",
};
