// Memory Drive reads shared by the web adapters: turn what a provider's own
// page returns (saved memories, custom instructions, history, one chat) into
// plain text. Pure functions, so they run in tests without a browser. The
// in-page fetches live in each adapter; nothing here sees a token or cookie.
import type { VendorMemory, VendorThreadSummary, VendorThreadText } from "@allternit/subscription-fabric-contracts";

/** Longest single memory or instruction kept. */
export const MAX_MEMORY_CHARS = 1_000;
/** Longest single message kept from a past chat. */
export const MAX_MESSAGE_CHARS = 4_000;
/** Most messages kept from one past chat (the latest ones). */
export const MAX_MESSAGES = 60;

const clean = (s: unknown, max: number): string =>
  typeof s === "string" ? s.replace(/\s+/g, " ").trim().slice(0, max) : "";

const iso = (v: unknown): string | null => {
  if (typeof v === "number" && Number.isFinite(v)) return new Date(v < 1e12 ? v * 1000 : v).toISOString();
  if (typeof v === "string" && !Number.isNaN(Date.parse(v))) return new Date(v).toISOString();
  return null;
};

/** A free-text memory block (one line or bullet per memory). */
export function splitMemoryText(text: string): string[] {
  return text
    .split(/\n+/)
    .map((l) => l.replace(/^\s*(?:[-*•]|\d+[.)])\s+/, "").trim())
    .filter((l) => l.length > 2 && !/^#+\s/.test(l))
    .map((l) => l.slice(0, MAX_MEMORY_CHARS));
}

// ─── ChatGPT ───────────────────────────────────────────────────────────────

/** What chatgpt.com's own memory and personalization endpoints answered. */
export interface ChatGPTMemoryRaw {
  memories: { ok: boolean; body: unknown };
  instructions: { ok: boolean; body: unknown };
}

const CHATGPT_INSTRUCTION_LABELS: Record<string, string> = {
  about_user_message: "About you",
  about_model_message: "How ChatGPT should respond",
  name_user_message: "What ChatGPT should call you",
  role_user_message: "Your work",
  traits_model_message: "Traits",
  other_user_message: "Anything else",
};

export function chatgptMemory(raw: ChatGPTMemoryRaw): VendorMemory {
  const mem = (raw.memories.body ?? {}) as { memories?: { content?: unknown; updated_at?: unknown }[] };
  const memories = (raw.memories.ok && Array.isArray(mem.memories) ? mem.memories : [])
    .map((m) => ({ text: clean(m.content, MAX_MEMORY_CHARS), updated_at: iso(m.updated_at) }))
    .filter((m) => m.text);
  const ins = (raw.instructions.ok ? raw.instructions.body : null) as Record<string, unknown> | null;
  const instructions =
    ins && ins.enabled !== false
      ? Object.entries(CHATGPT_INSTRUCTION_LABELS)
          .map(([key, label]) => ({ label, text: clean(ins[key], MAX_MEMORY_CHARS) }))
          .filter((i) => i.text)
      : [];
  return { available: raw.memories.ok || raw.instructions.ok, memories, instructions };
}

export function chatgptThreads(body: unknown): VendorThreadSummary[] {
  const items = (body as { items?: { id?: unknown; title?: unknown; update_time?: unknown }[] } | null)?.items;
  return (Array.isArray(items) ? items : [])
    .filter((t) => typeof t.id === "string")
    .map((t) => ({ id: t.id as string, title: clean(t.title, 200) || "Untitled", updated_at: iso(t.update_time) }));
}

/** One node of chatgpt.com's conversation tree, reduced in the page. */
export interface ChatGPTNode {
  id: string;
  parent: string | null;
  role: string | null;
  text: string;
  hidden: boolean;
}

/** The visible branch: from the current node back to the root. */
export function chatgptThreadText(id: string, title: unknown, updated: unknown, current: string | null, nodes: ChatGPTNode[]): VendorThreadText {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const path: ChatGPTNode[] = [];
  const seen = new Set<string>();
  for (let at = current ? byId.get(current) : undefined; at && !seen.has(at.id); at = at.parent ? byId.get(at.parent) : undefined) {
    seen.add(at.id);
    path.push(at);
  }
  const messages = path
    .reverse()
    .filter((n) => !n.hidden && (n.role === "user" || n.role === "assistant"))
    .map((n) => ({ role: n.role as "user" | "assistant", text: n.text.trim().slice(0, MAX_MESSAGE_CHARS) }))
    .filter((m) => m.text)
    .slice(-MAX_MESSAGES);
  return { id, title: clean(title, 200) || "Untitled", updated_at: iso(updated), messages };
}

// ─── Claude ────────────────────────────────────────────────────────────────

/** What claude.ai's own endpoints answered for memory and preferences. */
export interface ClaudeMemoryRaw {
  memory: { ok: boolean; body: unknown };
  profile: { ok: boolean; body: unknown };
}

export function claudeMemory(raw: ClaudeMemoryRaw): VendorMemory {
  const m = (raw.memory.ok ? raw.memory.body : null) as { memory?: unknown; memories?: { content?: unknown; text?: unknown; updated_at?: unknown }[] } | null;
  let memories: VendorMemory["memories"] = [];
  if (m && Array.isArray(m.memories)) {
    memories = m.memories.map((x) => ({ text: clean(x.content ?? x.text, MAX_MEMORY_CHARS), updated_at: iso(x.updated_at) })).filter((x) => x.text);
  } else if (m && typeof m.memory === "string") {
    memories = splitMemoryText(m.memory).map((text) => ({ text, updated_at: null }));
  }
  const p = (raw.profile.ok ? raw.profile.body : null) as { conversation_preferences?: unknown } | null;
  const prefs = clean(p?.conversation_preferences, MAX_MEMORY_CHARS);
  return {
    available: memories.length > 0 || Boolean(prefs),
    memories,
    instructions: prefs ? [{ label: "Personal preferences", text: prefs }] : [],
  };
}

export function claudeThreads(body: unknown): VendorThreadSummary[] {
  return (Array.isArray(body) ? (body as { uuid?: unknown; name?: unknown; updated_at?: unknown }[]) : [])
    .filter((t) => typeof t.uuid === "string")
    .map((t) => ({ id: t.uuid as string, title: clean(t.name, 200) || "Untitled", updated_at: iso(t.updated_at) }));
}

export function claudeThreadText(id: string, body: unknown): VendorThreadText | null {
  const j = body as { name?: unknown; updated_at?: unknown; chat_messages?: { sender?: unknown; text?: unknown; content?: { type?: unknown; text?: unknown }[] }[] } | null;
  if (!j || !Array.isArray(j.chat_messages)) return null;
  const messages = j.chat_messages
    .map((m) => {
      const blocks = Array.isArray(m.content) ? m.content.filter((b) => b.type === "text" && typeof b.text === "string").map((b) => b.text as string) : [];
      const text = (blocks.length ? blocks.join("\n") : typeof m.text === "string" ? m.text : "").trim().slice(0, MAX_MESSAGE_CHARS);
      const role = m.sender === "human" ? "user" : m.sender === "assistant" ? "assistant" : null;
      return role && text ? { role, text } : null;
    })
    .filter((m): m is { role: "user" | "assistant"; text: string } => m !== null)
    .slice(-MAX_MESSAGES);
  return { id, title: clean(j.name, 200) || "Untitled", updated_at: iso(j.updated_at), messages };
}
