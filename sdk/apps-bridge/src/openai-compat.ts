/**
 * window.openai compatibility metadata. The shim itself is runtime/openai-compat.js
 * (the W2 port); these helpers decide when a host should inject it. Same
 * logic as allternit-ai `src/lib/ai/mcp/openai-compat-meta.ts`.
 */

/** ChatGPT's Apps SDK resource MIME type (superseded by `text/html;profile=mcp-app`). */
export const SKYBRIDGE_MIME_TYPE = "text/html+skybridge";
/** `hostContext` key carrying the restored widget state to the shim. */
export const OPENAI_COMPAT_KEY = "x-openai-compat";
/** `structuredContent` key the shim uses to send widget state via ui/update-model-context. */
export const WIDGET_STATE_KEY = "openai/widgetState";
/** Explicit opt-in for a View that needs the shim but has no openai/* metadata. */
export const OPENAI_COMPAT_META_KEY = "allternit/openaiCompat";

const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

const hasOpenAiKey = (meta: unknown): boolean =>
  isRecord(meta) && Object.keys(meta).some((k) => k.startsWith("openai/") || k === OPENAI_COMPAT_META_KEY);

/** True when the tool/resource was authored for ChatGPT (or explicitly opts in). */
export function shouldInjectOpenAiCompat(input: { mimeType?: string; toolMeta?: unknown; resourceMeta?: unknown }): boolean {
  return input.mimeType === SKYBRIDGE_MIME_TYPE || hasOpenAiKey(input.toolMeta) || hasOpenAiKey(input.resourceMeta);
}
