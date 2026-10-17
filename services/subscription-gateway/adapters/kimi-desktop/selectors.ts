// Kimi desktop app AX selector pack "kimi-desktop-ax-v1" (route desktop-app-ax).
// /Applications/Kimi.app on this Mac (2026-10-17): bundle com.moonshot.kimichat v3.2.15, Electron 43.7.3 (productName
// kimi-desktop). Recon was bundle-only: the app was not running and was not launched, so EVERY selector is UNVERIFIED —
// guesses from the accessible names the kimi.com web UI gives its controls, in English and Chinese (the app follows the
// system language). The first consented live session must snapshot the real tree and correct these; an unresolved
// critical key reads as ADAPTER_DRIFT by design. Bump PACK_VERSION when the live UI drifts.
import { AX_SELECTOR_FORMAT, type AxKeySpec, type AxSelector, type AxSelectorPack, type ChatSkin } from "../_shared/ax/index.js";

export const KIMI_BUNDLE_ID = "com.moonshot.kimichat";
export const KIMI_BUNDLE_IDS = [KIMI_BUNDLE_ID];
export const PACK_VERSION = "kimi-desktop-ax-v1";
const s = (role: string, extra: Partial<AxSelector> = {}): AxSelector => ({ v: AX_SELECTOR_FORMAT, role, ...extra });
const key = (critical: boolean, ...alternatives: AxSelector[]): AxKeySpec => ({ critical, confidence: "unverified", alternatives });
const COMPOSER = "^(ask anything|message kimi|ask kimi|尽管问|有什么)";

export const KIMI_DESKTOP_AX_PACK: AxSelectorPack = {
  format: AX_SELECTOR_FORMAT,
  packVersion: PACK_VERSION,
  bundleId: KIMI_BUNDLE_ID,
  keys: {
    composer: key(true, s("AXTextArea", { label: COMPOSER }), s("AXTextField", { label: COMPOSER })),
    send: key(true, s("AXButton", { label: "^(send|发送)$" })),
    stop: key(false, s("AXButton", { label: "^(stop|stop generating|停止(生成|输出)?)$" })),
    newChat: key(true, s("AXButton", { label: "^(new chat|新建会话|新会话)$" }), s("AXLink", { label: "^(new chat|新建会话|新会话)$" })),
    userTurn: key(false, s("AXGroup", { label: "^(user message|you said|your message)" })),
    assistantTurn: key(false, s("AXGroup", { label: "^(kimi response|kimi said|assistant message)" })),
    alert: key(false, s("AXGroup", { subrole: "AXApplicationAlert" }), s("AXGroup", { label: "^(alert|notice|toast)" })),
    signIn: key(false, s("AXButton", { label: "^(log in|sign in|登录)$" })),
    modelPicker: key(false, s("AXPopUpButton", { label: "^(model|模型)" }), s("AXButton", { label: "^(model|模型)" })),
    modelOption: key(false, s("AXMenuItem", { path: [{ role: "AXMenu" }] })),
    threadRow: key(false, s("AXLink", { path: [{ role: "AXGroup", label: "^(history|chat history|历史会话|历史)" }] })),
    attach: key(false, s("AXButton", { label: "^(upload|attach|上传)" })),
  },
};

export const KIMI_DESKTOP_SKIN: ChatSkin = {
  appTitle: "Kimi", bundleId: KIMI_BUNDLE_ID, webArea: true,
  composer: { role: "AXTextArea", description: "Ask Anything" },
  send: "Send", stop: "Stop", newChat: "New Chat",
  userTurn: "User message", assistantTurn: "Kimi response", signIn: "Log in",
  modelPicker: { role: "AXPopUpButton", description: "Model" }, modelMenu: "Models",
  sidebar: "History", threadRole: "AXLink", attach: "Upload",
};

/** Kimi shows limits and checks in English or Chinese. */
export const KIMI_PATTERNS = {
  rateLimited: /usage limit|limit reached|reached (your|the) .*limit|too many (requests|messages)|rate limit|try again later|额度|上限|太频繁|稍后再试/i,
  blocked: /unusual activity|verify (that )?(you are|you're) (a )?human|captcha|security check|account (has been )?(suspended|disabled)|安全验证|人机验证|异常/i,
};
