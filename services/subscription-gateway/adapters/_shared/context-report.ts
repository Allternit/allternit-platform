import type { ContextReport } from "@allternit/subscription-fabric-contracts";

/**
 * Context reports for the signed-in chat apps: what fills a conversation's
 * context, from the raw JSON the vendor's own page endpoints return (read in
 * the page by each adapter's readContext). Pure, so the shapes are testable.
 * Tokens are chars/4, the estimate gizzi uses everywhere else.
 */

export const tokensOf = (value: unknown): number => {
  if (value === null || value === undefined) return 0;
  const text = typeof value === "string" ? value : JSON.stringify(value);
  return Math.ceil(text.length / 4);
};

function emptyRows(): ContextReport["rows"] {
  return { messages: 0, system_tools: 0, memory_files: 0, skills: 0, system_prompt: 0, mcp_tools: 0, mcp_instructions: 0 };
}

function addItem(list: { name: string; tokens: number }[], name: string, tokens: number) {
  const hit = list.find((item) => item.name === name);
  if (hit) hit.tokens += tokens;
  else list.push({ name, tokens });
}

// ── Claude (claude.ai) ──────────────────────────────────────────────────

export interface ClaudeContextRaw {
  /** /api/organizations/{org}/chat_conversations/{id}?tree=True&rendering_mode=messages */
  conversation: unknown;
  /** /api/organizations/{org}/memory */
  memory?: unknown;
  /** /api/organizations/{org}/projects/{project} */
  project?: unknown;
  /** /api/organizations/{org}/projects/{project}/docs */
  projectDocs?: unknown;
  /** /api/organizations/{org}/usage */
  usage?: unknown;
  /** /api/organizations/{org}/skills (when the account has skills) */
  skills?: unknown;
}

type ClaudeBlock = { type?: string; text?: unknown; thinking?: unknown; name?: unknown; input?: unknown; content?: unknown };
type ClaudeMessage = {
  sender?: string;
  text?: unknown;
  content?: ClaudeBlock[];
  attachments?: { file_name?: unknown; extracted_content?: unknown }[];
  files?: { file_name?: unknown }[];
  files_v2?: { file_name?: unknown }[];
};

export function claudeContextReport(raw: ClaudeContextRaw): ContextReport | null {
  const conv = raw.conversation as {
    model?: unknown;
    settings?: Record<string, unknown>;
    chat_messages?: ClaudeMessage[];
  } | null;
  if (!conv || !Array.isArray(conv.chat_messages)) return null;
  const rows = emptyRows();
  const systemTools: ContextReport["system_tools"] = [];
  const mcpTools: ContextReport["mcp_tools"] = [];
  const memoryFiles: ContextReport["memory_files"] = [];
  const skills: ContextReport["skills"] = [];

  // Messages: every block the conversation holds (text, tool calls and
  // results, thinking claude.ai keeps), plus attachments' extracted text.
  for (const message of conv.chat_messages) {
    const blocks = Array.isArray(message.content) ? message.content : [];
    if (blocks.length === 0) rows.messages += tokensOf(message.text);
    for (const block of blocks) {
      const tokens = tokensOf(block.text ?? block.thinking ?? block.input ?? block.content ?? "");
      rows.messages += tokens;
      // Tools the conversation used; MCP connector tools are named mcp__server__tool.
      if (block.type === "tool_use" && typeof block.name === "string") {
        const mcp = /^mcp__([^_][\w-]*?)__(.+)$/.exec(block.name);
        if (mcp) {
          if (!mcpTools.some((t) => t.name === block.name)) mcpTools.push({ name: block.name, server: mcp[1], tokens: 0 });
        } else addItem(systemTools, block.name, 0);
      }
    }
    for (const attachment of message.attachments ?? []) {
      rows.messages += tokensOf(attachment.extracted_content);
    }
  }

  // Memory: claude.ai's memory, the project's instructions and knowledge docs.
  const memory = raw.memory as { memory?: unknown; memories?: { content?: unknown; text?: unknown }[] } | null | undefined;
  const memoryText = Array.isArray(memory?.memories)
    ? memory!.memories.map((m) => (typeof m.content === "string" ? m.content : typeof m.text === "string" ? m.text : "")).join("\n")
    : typeof memory?.memory === "string"
      ? memory.memory
      : "";
  if (memoryText) memoryFiles.push({ path: "Claude memory", tokens: tokensOf(memoryText) });
  const project = raw.project as { name?: unknown; prompt_template?: unknown } | null | undefined;
  if (typeof project?.prompt_template === "string" && project.prompt_template) {
    memoryFiles.push({ path: `Project instructions${typeof project.name === "string" ? ` · ${project.name}` : ""}`, tokens: tokensOf(project.prompt_template) });
  }
  for (const doc of Array.isArray(raw.projectDocs) ? (raw.projectDocs as { file_name?: unknown; content?: unknown }[]) : []) {
    const tokens = tokensOf(doc.content);
    if (tokens > 0) memoryFiles.push({ path: `Project file · ${String(doc.file_name ?? "document")}`, tokens });
  }
  rows.memory_files = memoryFiles.reduce((sum, file) => sum + file.tokens, 0);

  // Skills the account has on (claude.ai lists each with its description).
  for (const skill of Array.isArray(raw.skills) ? (raw.skills as { name?: unknown; description?: unknown; enabled?: unknown }[]) : []) {
    if (typeof skill.name !== "string" || skill.enabled === false) continue;
    skills.push({ name: skill.name, tokens: tokensOf(`${skill.name}: ${String(skill.description ?? "")}`) });
  }
  rows.skills = skills.reduce((sum, skill) => sum + skill.tokens, 0);

  // Built-in tools switched on for the conversation (their schemas live server-side).
  for (const [key, value] of Object.entries(conv.settings ?? {})) {
    if (value === true && /^enabled_/.test(key)) addItem(systemTools, key.replace(/^enabled_/, "").replace(/_/g, " "), 0);
  }

  return {
    vendor: "claude",
    ...(typeof conv.model === "string" ? { model: conv.model } : {}),
    rows,
    memory_files: memoryFiles,
    mcp_tools: mcpTools,
    system_tools: systemTools,
    skills,
    // claude.ai does not show its system prompt or tool schemas.
    hidden: ["system_prompt", "system_tools", "mcp_tools"],
    limits: claudeLimits(raw.usage),
  };
}

function claudeLimits(usage: unknown): ContextReport["limits"] {
  const u = usage as Record<string, { utilization?: unknown; resets_at?: unknown } | null> | null | undefined;
  if (!u || typeof u !== "object") return undefined;
  const labels: Record<string, [string, string]> = {
    five_hour: ["5h", "5-hour"],
    seven_day: ["7d", "Weekly"],
    seven_day_opus: ["7d-opus", "Weekly · Opus"],
    seven_day_sonnet: ["7d-sonnet", "Weekly · Sonnet"],
  };
  const out: NonNullable<ContextReport["limits"]> = [];
  for (const [key, [id, label]] of Object.entries(labels)) {
    const w = u[key];
    if (!w || typeof w.utilization !== "number") continue;
    out.push({
      id,
      label,
      used_ratio: Math.min(1, Math.max(0, w.utilization > 1 ? w.utilization / 100 : w.utilization)),
      resets_at: typeof w.resets_at === "string" ? w.resets_at : null,
    });
  }
  return out.length > 0 ? out : undefined;
}

// ── ChatGPT (chatgpt.com) ───────────────────────────────────────────────

export interface ChatGPTContextRaw {
  /** /backend-api/conversation/{id} */
  conversation: unknown;
  /** /backend-api/memories?include_memory_entries=true */
  memories?: unknown;
  /** /backend-api/user_system_messages */
  instructions?: unknown;
}

type GPTNode = {
  id?: string;
  parent?: string | null;
  message?: {
    author?: { role?: string; name?: string | null };
    content?: { content_type?: string; parts?: unknown[]; text?: unknown; user_profile?: unknown; user_instructions?: unknown; model_set_context?: unknown };
    metadata?: { model_slug?: unknown; is_visually_hidden_from_conversation?: boolean; attachments?: unknown[] };
  } | null;
};

export function chatgptContextReport(raw: ChatGPTContextRaw): ContextReport | null {
  const conv = raw.conversation as { current_node?: string; mapping?: Record<string, GPTNode> } | null;
  if (!conv || !conv.mapping) return null;
  const rows = emptyRows();
  const systemTools: ContextReport["system_tools"] = [];
  const memoryFiles: ContextReport["memory_files"] = [];
  let model: string | undefined;
  let sawMemory = false;
  let sawInstructions = false;

  // The branch the conversation is on: current node back to the root.
  const path: GPTNode[] = [];
  const seen = new Set<string>();
  for (let at = conv.current_node ? conv.mapping[conv.current_node] : undefined; at && at.id && !seen.has(at.id); at = at.parent ? conv.mapping[at.parent] : undefined) {
    seen.add(at.id);
    path.push(at);
  }
  for (const node of path.reverse()) {
    const message = node.message;
    if (!message) continue;
    const content = message.content ?? {};
    const role = message.author?.role;
    if (typeof message.metadata?.model_slug === "string") model = message.metadata.model_slug;
    // The hidden context messages: custom instructions and saved memories.
    if (content.content_type === "user_editable_context") {
      const tokens = tokensOf([content.user_profile, content.user_instructions].filter(Boolean).join("\n"));
      if (tokens > 0) memoryFiles.push({ path: "Custom instructions", tokens });
      sawInstructions = true;
      continue;
    }
    if (content.content_type === "model_editable_context") {
      const tokens = tokensOf(content.model_set_context);
      if (tokens > 0) memoryFiles.push({ path: "Saved memories", tokens });
      sawMemory = true;
      continue;
    }
    if (role === "system" && message.metadata?.is_visually_hidden_from_conversation) continue;
    const tokens = tokensOf(content.parts ?? content.text ?? "");
    rows.messages += tokens;
    // A tool's turn (web.run, python, image_gen…): the tool was in context.
    if (role === "tool" && message.author?.name) addItem(systemTools, message.author.name, 0);
  }

  // Memory and custom instructions from their own endpoints when the
  // conversation did not carry them inline.
  if (!sawMemory) {
    const mem = raw.memories as { memories?: { content?: unknown }[] } | null | undefined;
    const text = Array.isArray(mem?.memories) ? mem!.memories.map((m) => String(m.content ?? "")).join("\n") : "";
    if (text.trim()) memoryFiles.push({ path: "Saved memories", tokens: tokensOf(text) });
  }
  if (!sawInstructions) {
    const ins = raw.instructions as { enabled?: unknown; about_user_message?: unknown; about_model_message?: unknown } | null | undefined;
    const text = ins && ins.enabled !== false ? [ins.about_user_message, ins.about_model_message].filter((v) => typeof v === "string" && v).join("\n") : "";
    if (text) memoryFiles.push({ path: "Custom instructions", tokens: tokensOf(text) });
  }
  rows.memory_files = memoryFiles.reduce((sum, file) => sum + file.tokens, 0);

  return {
    vendor: "chatgpt",
    ...(model ? { model } : {}),
    rows,
    memory_files: memoryFiles,
    mcp_tools: [],
    system_tools: systemTools,
    skills: [],
    // OpenAI does not show ChatGPT's system prompt or tool schemas.
    hidden: ["system_prompt", "system_tools", "mcp_tools"],
  };
}
