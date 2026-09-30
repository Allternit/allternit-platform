// Claude Desktop AX selector pack "claude-ax-v1". NOTHING here was checked against the running app: every selector is
// UNVERIFIED (role/label guesses from how Chromium exposes claude.ai's accessible names). The first consented live session
// (Accessibility granted to the helper, then Eoj's OK) must snapshot the real tree and correct these; until then an
// unresolved critical key reads as ADAPTER_DRIFT by design. Bump PACK_VERSION when the live UI drifts.
import { AX_SELECTOR_FORMAT, type AxKeySpec, type AxSelector, type AxSelectorPack } from "../../_shared/ax/index.js";

export const CLAUDE_BUNDLE_ID = "com.anthropic.claudefordesktop"; // unverified
export const PACK_VERSION = "claude-ax-v1";
const s = (role: string, extra: Partial<AxSelector> = {}): AxSelector => ({ v: AX_SELECTOR_FORMAT, role, ...extra });
const key = (critical: boolean, ...alternatives: AxSelector[]): AxKeySpec => ({ critical, confidence: "unverified", alternatives });

export const CLAUDE_AX_PACK: AxSelectorPack = {
  format: AX_SELECTOR_FORMAT,
  packVersion: PACK_VERSION,
  bundleId: CLAUDE_BUNDLE_ID,
  keys: {
    composer: key(true, s("AXTextArea", { label: "^prompt$" }), s("AXTextArea", { label: "write your prompt|reply to claude|message claude" }), s("AXTextField", { label: "write your prompt|reply to claude|message claude" })),
    userTurn: key(false, s("AXGroup", { label: "^(your message|you said|user message)" })),
    assistantTurn: key(false, s("AXGroup", { label: "^(claude response|claude said|assistant message)" })),
    alert: key(false, s("AXGroup", { subrole: "AXApplicationAlert" }), s("AXGroup", { label: "^(alert|notice|banner)" })),
    approval: key(false, s("AXGroup", { label: "permission request|approval request" }), s("AXSheet", { label: "permission|approval" })),
    toolCue: key(false, s("AXGroup", { label: "^tool use" })),
    artifactCue: key(false, s("AXGroup", { label: "^artifact" })),
    coworkTab: key(false, s("AXRadioButton", { label: "^(chat and )?cowork$" }), s("AXTab", { label: "^cowork$" })),
  },
};
