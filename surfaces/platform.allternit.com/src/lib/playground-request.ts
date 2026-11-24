/**
 * Pure request logic for the Playground: building the OpenAI-compatible body,
 * assembling a streamed response, pricing a call, and exporting code.
 * Nothing here touches the network, storage or React.
 */

import type {
  ChatCompletionChunk,
  ChatCompletionMessage,
  ChatToolCall,
} from "@/lib/console-gateway";

export type ResponseFormatKind = "text" | "json_object" | "json_schema";

export interface PlaygroundParams {
  temperature: number;
  topP: number | null;
  maxTokens: number;
  /** One stop sequence per line. */
  stop: string;
  presencePenalty: number | null;
  frequencyPenalty: number | null;
  seed: number | null;
  stream: boolean;
  /** JSON array of OpenAI tool definitions, or empty. */
  toolsJson: string;
  /** "auto" | "none" | "required" | "fn:<name>" */
  toolChoice: string;
  responseFormat: ResponseFormatKind;
  schemaName: string;
  schemaJson: string;
}

export const DEFAULT_PARAMS: PlaygroundParams = {
  temperature: 0.7,
  topP: null,
  maxTokens: 4096,
  stop: "",
  presencePenalty: null,
  frequencyPenalty: null,
  seed: null,
  stream: true,
  toolsJson: "",
  toolChoice: "auto",
  responseFormat: "text",
  schemaName: "response",
  schemaJson: "",
};

export interface BuildResult {
  body: Record<string, unknown>;
  /** Human-readable problems with the tools or schema JSON. */
  errors: string[];
}

function parseJson(label: string, text: string, errors: string[]): unknown {
  try {
    return JSON.parse(text);
  } catch (err) {
    errors.push(`${label} is not valid JSON: ${err instanceof Error ? err.message : "parse error"}`);
    return undefined;
  }
}

export function buildRequestBody(
  model: string,
  messages: ChatCompletionMessage[],
  p: PlaygroundParams
): BuildResult {
  const errors: string[] = [];
  const body: Record<string, unknown> = { model, messages };

  body.temperature = p.temperature;
  if (p.topP != null) body.top_p = p.topP;
  body.max_tokens = p.maxTokens;
  const stops = p.stop
    .split("\n")
    .map((s) => s.replace(/\r$/, ""))
    .filter((s) => s.length > 0);
  if (stops.length > 0) body.stop = stops;
  if (p.presencePenalty != null) body.presence_penalty = p.presencePenalty;
  if (p.frequencyPenalty != null) body.frequency_penalty = p.frequencyPenalty;
  if (p.seed != null) body.seed = p.seed;

  if (p.toolsJson.trim()) {
    const tools = parseJson("Tools", p.toolsJson, errors);
    if (tools !== undefined) {
      if (!Array.isArray(tools)) {
        errors.push("Tools must be a JSON array of function definitions.");
      } else if (tools.length > 0) {
        body.tools = tools;
        if (p.toolChoice.startsWith("fn:")) {
          body.tool_choice = { type: "function", function: { name: p.toolChoice.slice(3) } };
        } else if (p.toolChoice !== "auto") {
          body.tool_choice = p.toolChoice;
        }
      }
    }
  }

  if (p.responseFormat === "json_object") {
    body.response_format = { type: "json_object" };
  } else if (p.responseFormat === "json_schema") {
    const schema = parseJson("Schema", p.schemaJson || "{}", errors);
    if (schema !== undefined) {
      body.response_format = {
        type: "json_schema",
        json_schema: { name: p.schemaName.trim() || "response", strict: true, schema },
      };
    }
  }

  if (p.stream) {
    body.stream = true;
    body.stream_options = { include_usage: true };
  }
  return { body, errors };
}

/** Names of the functions in a tools JSON array (for the tool_choice picker). */
export function toolNames(toolsJson: string): string[] {
  try {
    const parsed = JSON.parse(toolsJson);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .map((t) => (t && typeof t === "object" ? (t as { function?: { name?: unknown } }).function?.name : undefined))
      .filter((n): n is string => typeof n === "string" && n.length > 0);
  } catch {
    return [];
  }
}

export function imageContent(
  text: string,
  dataUrls: string[]
): ChatCompletionMessage["content"] {
  if (dataUrls.length === 0) return text;
  return [
    ...(text ? [{ type: "text" as const, text }] : []),
    ...dataUrls.map((url) => ({ type: "image_url" as const, image_url: { url } })),
  ];
}

// ─── Streaming assembly ─────────────────────────────────────────────────────

export interface AssembledResponse {
  id?: string;
  model?: string;
  content: string;
  toolCalls: ChatToolCall[];
  finishReason: string | null;
  usage: { prompt_tokens?: number; completion_tokens?: number; total_tokens?: number } | null;
}

export function newAssembly(): AssembledResponse {
  return { content: "", toolCalls: [], finishReason: null, usage: null };
}

/** Fold one SSE chunk into the running response (pure: returns a new object). */
export function mergeChunk(acc: AssembledResponse, chunk: ChatCompletionChunk): AssembledResponse {
  const next: AssembledResponse = { ...acc, toolCalls: acc.toolCalls.map((t) => ({ ...t, function: { ...t.function } })) };
  if (chunk.id) next.id = chunk.id;
  if (chunk.model) next.model = chunk.model;
  if (chunk.usage) next.usage = chunk.usage;
  const choice = chunk.choices?.[0];
  if (!choice) return next;
  if (choice.finish_reason) next.finishReason = choice.finish_reason;
  const delta = choice.delta;
  if (delta?.content) next.content += delta.content;
  for (const tc of delta?.tool_calls ?? []) {
    const idx = tc.index ?? 0;
    const cur = next.toolCalls[idx] ?? { id: "", type: "function" as const, function: { name: "", arguments: "" } };
    if (tc.id) cur.id = tc.id;
    if (tc.function?.name) cur.function.name += tc.function.name;
    if (tc.function?.arguments) cur.function.arguments += tc.function.arguments;
    next.toolCalls[idx] = cur;
  }
  next.toolCalls = next.toolCalls.filter(Boolean);
  return next;
}

/** Read a non-streamed chat completion body into the same shape. */
export function readCompletion(json: unknown): AssembledResponse {
  const j = (json ?? {}) as {
    id?: string;
    model?: string;
    usage?: AssembledResponse["usage"];
    choices?: Array<{
      finish_reason?: string | null;
      message?: { content?: string | null; tool_calls?: ChatToolCall[] };
    }>;
  };
  const choice = j.choices?.[0];
  return {
    id: j.id,
    model: j.model,
    content: choice?.message?.content ?? "",
    toolCalls: choice?.message?.tool_calls ?? [],
    finishReason: choice?.finish_reason ?? null,
    usage: j.usage ?? null,
  };
}

/** The response JSON the inspector shows for a streamed call. */
export function assembledToJson(a: AssembledResponse): Record<string, unknown> {
  return {
    id: a.id,
    object: "chat.completion",
    model: a.model,
    choices: [
      {
        index: 0,
        message: {
          role: "assistant",
          content: a.content || null,
          ...(a.toolCalls.length > 0 ? { tool_calls: a.toolCalls } : {}),
        },
        finish_reason: a.finishReason,
      },
    ],
    usage: a.usage ?? undefined,
  };
}

// ─── Cost ───────────────────────────────────────────────────────────────────

/** USD for a call from per-token prices; null when the model has no prices. */
export function computeCostUsd(
  usage: { prompt_tokens?: number; completion_tokens?: number } | null,
  promptPrice?: number,
  completionPrice?: number
): number | null {
  if (!usage || (promptPrice == null && completionPrice == null)) return null;
  return (usage.prompt_tokens ?? 0) * (promptPrice ?? 0) + (usage.completion_tokens ?? 0) * (completionPrice ?? 0);
}

export function formatUsd(value: number): string {
  if (value === 0) return "$0";
  if (value < 0.0001) return `<$0.0001`;
  return `$${value.toFixed(value < 0.01 ? 5 : 4)}`;
}

// ─── Export ─────────────────────────────────────────────────────────────────

/** The export always shows this placeholder, never the minted key. */
export const KEY_PLACEHOLDER = "$ALLTERNIT_API_KEY";

export type ExportLanguage = "curl" | "python" | "typescript";

function shellQuote(text: string): string {
  return `'${text.replace(/'/g, `'\\''`)}'`;
}

export function exportRequest(
  language: ExportLanguage,
  body: Record<string, unknown>,
  baseUrl: string
): string {
  const b = body;
  const root = baseUrl.replace(/\/+$/, "");
  if (language === "curl") {
    return [
      `curl ${root}/v1/chat/completions \\`,
      `  -H "Authorization: Bearer ${KEY_PLACEHOLDER}" \\`,
      `  -H "Content-Type: application/json" \\`,
      `  -d ${shellQuote(JSON.stringify(b, null, 2))}`,
    ].join("\n");
  }
  const { messages, model, stream, stream_options: _so, ...rest } = b as {
    messages: unknown;
    model: unknown;
    stream?: boolean;
    stream_options?: unknown;
    [k: string]: unknown;
  };
  if (language === "python") {
    const pyArgs = Object.entries(rest)
      .map(([k, v]) => `    ${k}=${toPython(v, 4)},`)
      .join("\n");
    return [
      "import os",
      "from openai import OpenAI",
      "",
      "client = OpenAI(",
      `    base_url="${root}/v1",`,
      `    api_key=os.environ["ALLTERNIT_API_KEY"],  # ${KEY_PLACEHOLDER}`,
      ")",
      "",
      "response = client.chat.completions.create(",
      `    model=${toPython(model, 4)},`,
      `    messages=${toPython(messages, 4)},`,
      pyArgs,
      stream ? "    stream=True," : "",
      ")",
      "",
      stream
        ? 'for chunk in response:\n    if chunk.choices and chunk.choices[0].delta.content:\n        print(chunk.choices[0].delta.content, end="")'
        : "print(response.choices[0].message)",
    ]
      .filter((l, i, arr) => !(l === "" && arr[i - 1] === ""))
      .join("\n")
      .replace(/\n\n\)/, "\n)");
  }
  const tsArgs = Object.entries(rest)
    .map(([k, v]) => `  ${k}: ${indent(JSON.stringify(v, null, 2), 2)},`)
    .join("\n");
  return [
    'import OpenAI from "openai";',
    "",
    "const client = new OpenAI({",
    `  baseURL: "${root}/v1",`,
    `  apiKey: process.env.ALLTERNIT_API_KEY, // ${KEY_PLACEHOLDER}`,
    "});",
    "",
    `const response = await client.chat.completions.create({`,
    `  model: ${JSON.stringify(model)},`,
    `  messages: ${indent(JSON.stringify(messages, null, 2), 2)},`,
    tsArgs,
    stream ? "  stream: true," : "",
    "});",
    "",
    stream
      ? "for await (const chunk of response) {\n  process.stdout.write(chunk.choices[0]?.delta?.content ?? \"\");\n}"
      : "console.log(response.choices[0].message);",
  ]
    .filter((l, i, arr) => !(l === "" && arr[i - 1] === ""))
    .join("\n")
    .replace(/\n\n\}\);/, "\n});");
}

function indent(text: string, spaces: number): string {
  const pad = " ".repeat(spaces);
  return text
    .split("\n")
    .map((line, i) => (i === 0 ? line : pad + line))
    .join("\n");
}

/** JSON value to a Python literal. */
export function toPython(value: unknown, spaces = 0): string {
  if (value === null || value === undefined) return "None";
  if (value === true) return "True";
  if (value === false) return "False";
  if (typeof value === "number") return String(value);
  if (typeof value === "string") return JSON.stringify(value);
  const pad = " ".repeat(spaces + 4);
  const end = " ".repeat(spaces);
  if (Array.isArray(value)) {
    if (value.length === 0) return "[]";
    return `[\n${value.map((v) => `${pad}${toPython(v, spaces + 4)},`).join("\n")}\n${end}]`;
  }
  const entries = Object.entries(value as Record<string, unknown>);
  if (entries.length === 0) return "{}";
  return `{\n${entries
    .map(([k, v]) => `${pad}${JSON.stringify(k)}: ${toPython(v, spaces + 4)},`)
    .join("\n")}\n${end}}`;
}

/** Strip image data URLs so a saved prompt or export stays small. */
export function redactImages(messages: ChatCompletionMessage[]): ChatCompletionMessage[] {
  return messages.map((m) =>
    Array.isArray(m.content)
      ? {
          ...m,
          content: m.content.map((part) =>
            part.type === "image_url" && part.image_url.url.startsWith("data:")
              ? { type: "image_url" as const, image_url: { url: "data:image/png;base64,<image>" } }
              : part
          ),
        }
      : m
  );
}

// ─── Loading a request body back into the editor (replay, saved prompts) ────

export interface EditableLike {
  id: string;
  role: "user" | "assistant" | "tool";
  content: string;
  images?: string[];
  toolCalls?: ChatToolCall[];
  toolCallId?: string;
}

let idCounter = 0;
function nextId(): string {
  idCounter += 1;
  return `msg-${Date.now()}-${idCounter}`;
}

/** Split a messages array into the system prompt and editable rows. */
export function messagesToEditable(messages: unknown): { systemPrompt: string; messages: EditableLike[] } {
  const out: EditableLike[] = [];
  const system: string[] = [];
  for (const raw of Array.isArray(messages) ? messages : []) {
    if (!raw || typeof raw !== "object") continue;
    const m = raw as ChatCompletionMessage;
    let text = "";
    const images: string[] = [];
    if (typeof m.content === "string") text = m.content;
    else if (Array.isArray(m.content)) {
      for (const part of m.content) {
        if (part.type === "text") text += (text ? "\n" : "") + part.text;
        else if (part.type === "image_url" && !part.image_url.url.includes("<image>")) images.push(part.image_url.url);
      }
    }
    if (m.role === "system") system.push(text);
    else if (m.role === "user") out.push({ id: nextId(), role: "user", content: text, ...(images.length ? { images } : {}) });
    else if (m.role === "assistant")
      out.push({ id: nextId(), role: "assistant", content: text, ...(m.tool_calls?.length ? { toolCalls: m.tool_calls } : {}) });
    else if (m.role === "tool") out.push({ id: nextId(), role: "tool", content: text, toolCallId: m.tool_call_id });
  }
  return { systemPrompt: system.join("\n\n"), messages: out };
}

/** Params recovered from a stored request body; unknown keys keep the defaults. */
export function paramsFromBody(body: Record<string, unknown>, base: PlaygroundParams = DEFAULT_PARAMS): PlaygroundParams {
  const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
  const p: PlaygroundParams = { ...base };
  p.temperature = num(body.temperature) ?? base.temperature;
  p.topP = num(body.top_p);
  p.maxTokens = num(body.max_tokens) ?? base.maxTokens;
  p.presencePenalty = num(body.presence_penalty);
  p.frequencyPenalty = num(body.frequency_penalty);
  p.seed = num(body.seed);
  p.stream = body.stream === true;
  p.stop = Array.isArray(body.stop) ? body.stop.filter((s) => typeof s === "string").join("\n") : typeof body.stop === "string" ? body.stop : "";
  p.toolsJson = Array.isArray(body.tools) && body.tools.length > 0 ? JSON.stringify(body.tools, null, 2) : "";
  const tc = body.tool_choice as unknown;
  p.toolChoice =
    typeof tc === "string"
      ? tc
      : tc && typeof tc === "object" && typeof (tc as { function?: { name?: string } }).function?.name === "string"
        ? `fn:${(tc as { function: { name: string } }).function.name}`
        : "auto";
  const rf = body.response_format as { type?: string; json_schema?: { name?: string; schema?: unknown } } | undefined;
  if (rf?.type === "json_object") p.responseFormat = "json_object";
  else if (rf?.type === "json_schema") {
    p.responseFormat = "json_schema";
    p.schemaName = rf.json_schema?.name ?? "response";
    p.schemaJson = JSON.stringify(rf.json_schema?.schema ?? {}, null, 2);
  } else p.responseFormat = "text";
  return p;
}

// ─── API modes: Chat Completions, Responses, Messages ───────────────────────

/** Which Platform API route the Playground calls. */
export type PlaygroundMode = "chat" | "responses" | "messages";

export const PLAYGROUND_MODES: { id: PlaygroundMode; label: string }[] = [
  { id: "chat", label: "Chat completions" },
  { id: "responses", label: "Responses" },
  { id: "messages", label: "Messages" },
];

export const MODE_PATHS: Record<PlaygroundMode, string> = {
  chat: "/v1/chat/completions",
  responses: "/v1/responses",
  messages: "/v1/messages",
};

export function isPlaygroundMode(value: unknown): value is PlaygroundMode {
  return value === "chat" || value === "responses" || value === "messages";
}

export interface ModeBuildResult extends BuildResult {
  /** Editor settings this route does not take, so they are not sent. */
  ignored: string[];
}

export interface ModeBuildOptions {
  /** Responses only: continue this stored response (sends only the new turn). */
  previousResponseId?: string | null;
}

function partsText(content: ChatCompletionMessage["content"]): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((p): p is { type: "text"; text: string } => p.type === "text")
    .map((p) => p.text)
    .join("\n");
}

function ignoredParams(p: PlaygroundParams): string[] {
  const out: string[] = [];
  if (p.topP != null) out.push("top_p");
  if (p.stop.trim()) out.push("stop sequences");
  if (p.presencePenalty != null) out.push("presence penalty");
  if (p.frequencyPenalty != null) out.push("frequency penalty");
  if (p.seed != null) out.push("seed");
  if (p.responseFormat !== "text") out.push("response format");
  return out;
}

/** Chat-style tools JSON → validated array of `{name, description?, parameters?}`. */
function functionTools(p: PlaygroundParams, errors: string[]): Array<{ name: string; description?: string; parameters?: unknown }> {
  if (!p.toolsJson.trim()) return [];
  const tools = parseJson("Tools", p.toolsJson, errors);
  if (tools === undefined) return [];
  if (!Array.isArray(tools)) {
    errors.push("Tools must be a JSON array of function definitions.");
    return [];
  }
  return tools.map((t) => {
    const fn = (t && typeof t === "object" ? ((t as { function?: unknown }).function ?? t) : {}) as {
      name?: unknown;
      description?: unknown;
      parameters?: unknown;
      input_schema?: unknown;
    };
    return {
      name: typeof fn.name === "string" ? fn.name : "",
      ...(typeof fn.description === "string" ? { description: fn.description } : {}),
      ...(fn.parameters !== undefined || fn.input_schema !== undefined
        ? { parameters: fn.parameters ?? fn.input_schema }
        : {}),
    };
  });
}

function splitSystem(messages: ChatCompletionMessage[]): { system: string; rest: ChatCompletionMessage[] } {
  const system: string[] = [];
  const rest: ChatCompletionMessage[] = [];
  for (const m of messages) {
    if (m.role === "system") system.push(partsText(m.content));
    else rest.push(m);
  }
  return { system: system.filter(Boolean).join("\n\n"), rest };
}

/** Chat messages → Responses input items. */
export function toResponsesInput(messages: ChatCompletionMessage[]): Record<string, unknown>[] {
  const out: Record<string, unknown>[] = [];
  for (const m of messages) {
    if (m.role === "tool") {
      out.push({ type: "function_call_output", call_id: m.tool_call_id ?? "", output: partsText(m.content) });
    } else if (m.role === "assistant") {
      const text = partsText(m.content);
      if (text) out.push({ role: "assistant", content: text });
      for (const t of m.tool_calls ?? []) {
        out.push({ type: "function_call", call_id: t.id, name: t.function.name, arguments: t.function.arguments || "{}" });
      }
    } else if (m.role === "user") {
      if (Array.isArray(m.content)) {
        out.push({
          role: "user",
          content: m.content.map((part) =>
            part.type === "text"
              ? { type: "input_text", text: part.text }
              : { type: "input_image", image_url: part.image_url.url }
          ),
        });
      } else {
        out.push({ role: "user", content: m.content ?? "" });
      }
    }
  }
  return out;
}

function imageBlock(url: string): Record<string, unknown> {
  const m = /^data:(image\/[a-z0-9.+-]+);base64,(.*)$/i.exec(url);
  return m
    ? { type: "image", source: { type: "base64", media_type: m[1], data: m[2] } }
    : { type: "image", source: { type: "url", url } };
}

function parseArgs(text: string): unknown {
  try {
    return text.trim() ? JSON.parse(text) : {};
  } catch {
    return {};
  }
}

/** Chat messages → Anthropic-shaped messages (tool results grouped into one user turn). */
export function toAnthropicMessages(messages: ChatCompletionMessage[]): Record<string, unknown>[] {
  const out: Array<{ role: string; content: string | Record<string, unknown>[] }> = [];
  for (const m of messages) {
    if (m.role === "tool") {
      const block = { type: "tool_result", tool_use_id: m.tool_call_id ?? "", content: partsText(m.content) };
      const last = out[out.length - 1];
      if (last && last.role === "user" && Array.isArray(last.content) && last.content.every((b) => b.type === "tool_result")) {
        last.content.push(block);
      } else {
        out.push({ role: "user", content: [block] });
      }
    } else if (m.role === "assistant") {
      const text = partsText(m.content);
      if (!m.tool_calls?.length) {
        if (text) out.push({ role: "assistant", content: text });
        continue;
      }
      out.push({
        role: "assistant",
        content: [
          ...(text ? [{ type: "text", text }] : []),
          ...m.tool_calls.map((t) => ({ type: "tool_use", id: t.id, name: t.function.name, input: parseArgs(t.function.arguments) })),
        ],
      });
    } else if (m.role === "user") {
      if (Array.isArray(m.content)) {
        out.push({
          role: "user",
          content: m.content.map((part) =>
            part.type === "text" ? { type: "text", text: part.text } : imageBlock(part.image_url.url)
          ),
        });
      } else {
        out.push({ role: "user", content: m.content ?? "" });
      }
    }
  }
  return out;
}

/**
 * The request body for `mode` from the same editor state. Chat mode is exactly
 * buildRequestBody. The system prompt becomes Responses `instructions` or
 * Messages `system`; max tokens becomes `max_output_tokens` / `max_tokens`.
 */
export function buildModeRequestBody(
  mode: PlaygroundMode,
  model: string,
  messages: ChatCompletionMessage[],
  p: PlaygroundParams,
  opts: ModeBuildOptions = {}
): ModeBuildResult {
  if (mode === "chat") return { ...buildRequestBody(model, messages, p), ignored: [] };
  const errors: string[] = [];
  const ignored = ignoredParams(p);
  const { system, rest } = splitSystem(messages);
  const tools = functionTools(p, errors);

  if (mode === "responses") {
    const body: Record<string, unknown> = { model };
    let turn = rest;
    const previous = opts.previousResponseId?.trim();
    if (previous) {
      // The stored conversation holds the history: send only what follows the last assistant turn.
      const lastAssistant = rest.map((m) => m.role).lastIndexOf("assistant");
      turn = rest.slice(lastAssistant + 1);
      if (turn.length === 0) errors.push(`Add a new message after the last assistant turn to continue ${previous}.`);
    }
    body.input = toResponsesInput(turn);
    if (system) body.instructions = system;
    body.max_output_tokens = p.maxTokens;
    body.temperature = p.temperature;
    if (tools.length > 0) {
      body.tools = tools.map((t) => ({ type: "function", ...t }));
      if (p.toolChoice.startsWith("fn:")) body.tool_choice = { type: "function", name: p.toolChoice.slice(3) };
      else if (p.toolChoice !== "auto") body.tool_choice = p.toolChoice;
    }
    if (previous) body.previous_response_id = previous;
    if (p.stream) body.stream = true;
    return { body, errors, ignored };
  }

  const body: Record<string, unknown> = { model, max_tokens: p.maxTokens };
  if (system) body.system = system;
  body.messages = toAnthropicMessages(rest);
  body.temperature = p.temperature;
  if (tools.length > 0) {
    body.tools = tools.map(({ name, description, parameters }) => ({
      name,
      ...(description ? { description } : {}),
      input_schema: parameters && typeof parameters === "object" ? parameters : { type: "object", properties: {} },
    }));
    if (p.toolChoice.startsWith("fn:")) body.tool_choice = { type: "tool", name: p.toolChoice.slice(3) };
    else if (p.toolChoice === "required") body.tool_choice = { type: "any" };
    else if (p.toolChoice === "none") body.tool_choice = { type: "none" };
  }
  if (p.stream) body.stream = true;
  return { body, errors, ignored };
}

// ─── Parsing Responses and Messages replies ─────────────────────────────────

/** A streamed or whole reply from any mode, in the output pane's terms. */
export interface ModeAssembly extends AssembledResponse {
  /** Responses `incomplete_details.reason`. */
  incompleteReason: string | null;
  /** A failure reported inside the stream (`response.failed`, `error`). */
  error: string | null;
  /** The final object for the inspector, when the server sent one. */
  raw: unknown;
  /** Messages streams: tool_use id per content block index. */
  blockIds?: Record<number, string>;
}

export function newModeAssembly(): ModeAssembly {
  return { ...newAssembly(), incompleteReason: null, error: null, raw: null };
}

function cloneAssembly(acc: ModeAssembly): ModeAssembly {
  return { ...acc, toolCalls: acc.toolCalls.map((t) => ({ ...t, function: { ...t.function } })) };
}

type TokenUsage = { input_tokens?: number; output_tokens?: number } | null | undefined;

function mapUsage(u: TokenUsage): AssembledResponse["usage"] {
  if (!u || (u.input_tokens == null && u.output_tokens == null)) return null;
  const prompt = u.input_tokens ?? 0;
  const completion = u.output_tokens ?? 0;
  return { prompt_tokens: prompt, completion_tokens: completion, total_tokens: prompt + completion };
}

function errorText(e: unknown): string | null {
  if (!e) return null;
  if (typeof e === "string") return e;
  const o = e as { message?: unknown; error?: unknown };
  if (typeof o.message === "string") return o.message;
  return errorText(o.error);
}

/** Read a Responses `response` object (non-stream body, or the final stream event). */
export function readResponseObject(json: unknown): ModeAssembly {
  const r = (json ?? {}) as {
    id?: string;
    model?: string;
    status?: string;
    output?: Array<Record<string, unknown>>;
    usage?: TokenUsage;
    incomplete_details?: { reason?: string } | null;
    error?: unknown;
  };
  let content = "";
  const toolCalls: ChatToolCall[] = [];
  for (const item of r.output ?? []) {
    if (item.type === "message" && Array.isArray(item.content)) {
      for (const part of item.content as Array<{ type?: string; text?: string }>) {
        if (part.type === "output_text" && typeof part.text === "string") content += part.text;
      }
    } else if (item.type === "function_call") {
      toolCalls.push({
        id: String(item.call_id ?? item.id ?? ""),
        type: "function",
        function: { name: String(item.name ?? ""), arguments: typeof item.arguments === "string" ? item.arguments : JSON.stringify(item.arguments ?? {}) },
      });
    }
  }
  return {
    id: r.id,
    model: r.model ?? undefined,
    content,
    toolCalls,
    finishReason: r.status ?? null,
    usage: mapUsage(r.usage),
    incompleteReason: r.incomplete_details?.reason ?? null,
    error: r.status === "failed" ? (errorText(r.error) ?? "The response failed.") : null,
    raw: json,
  };
}

/** Fold one Responses SSE event into the running reply (pure). */
export function mergeResponsesEvent(acc: ModeAssembly, event: string, data: unknown): ModeAssembly {
  const d = (data ?? {}) as Record<string, unknown>;
  const name = typeof d.type === "string" ? d.type : event;
  switch (name) {
    case "response.created": {
      const r = (d.response ?? {}) as { id?: string; model?: string };
      return { ...cloneAssembly(acc), id: r.id ?? acc.id, model: r.model ?? acc.model };
    }
    case "response.output_text.delta":
      return { ...cloneAssembly(acc), content: acc.content + (typeof d.delta === "string" ? d.delta : "") };
    case "response.output_item.done": {
      const item = (d.item ?? {}) as Record<string, unknown>;
      if (item.type !== "function_call") return acc;
      const next = cloneAssembly(acc);
      const id = String(item.call_id ?? item.id ?? "");
      const call: ChatToolCall = {
        id,
        type: "function",
        function: { name: String(item.name ?? ""), arguments: typeof item.arguments === "string" ? item.arguments : "{}" },
      };
      const at = next.toolCalls.findIndex((t) => t.id === id);
      if (at >= 0) next.toolCalls[at] = call;
      else next.toolCalls.push(call);
      return next;
    }
    case "response.completed":
    case "response.incomplete":
    case "response.failed": {
      const final = readResponseObject(d.response);
      return {
        ...final,
        // Keep streamed text if the final object came without output.
        content: final.content || acc.content,
        toolCalls: final.toolCalls.length > 0 ? final.toolCalls : acc.toolCalls,
        id: final.id ?? acc.id,
        model: final.model ?? acc.model,
      };
    }
    case "error":
      return { ...cloneAssembly(acc), error: errorText(d) ?? "The stream reported an error." };
    default:
      return acc;
  }
}

/** Read an Anthropic-shaped message (non-stream body). */
export function readMessageObject(json: unknown): ModeAssembly {
  const m = (json ?? {}) as {
    id?: string;
    model?: string;
    content?: Array<{ type?: string; text?: string; id?: string; name?: string; input?: unknown }>;
    stop_reason?: string | null;
    usage?: TokenUsage;
  };
  let content = "";
  const toolCalls: ChatToolCall[] = [];
  for (const b of m.content ?? []) {
    if (b.type === "text" && typeof b.text === "string") content += b.text;
    else if (b.type === "tool_use") {
      toolCalls.push({ id: b.id ?? "", type: "function", function: { name: b.name ?? "", arguments: JSON.stringify(b.input ?? {}) } });
    }
  }
  return {
    id: m.id,
    model: m.model,
    content,
    toolCalls,
    finishReason: m.stop_reason ?? null,
    usage: mapUsage(m.usage),
    incompleteReason: null,
    error: null,
    raw: json,
  };
}

/** Fold one Messages SSE event into the running reply (pure). Tool blocks are keyed by stream index. */
export function mergeMessagesEvent(acc: ModeAssembly, event: string, data: unknown): ModeAssembly {
  const d = (data ?? {}) as Record<string, unknown>;
  const name = event === "error" ? "error" : typeof d.type === "string" ? d.type : event;
  const blockIndex = typeof d.index === "number" ? d.index : 0;
  const blockIds = acc.blockIds ?? {};
  switch (name) {
    case "message_start": {
      const msg = (d.message ?? {}) as { id?: string; model?: string };
      return { ...cloneAssembly(acc), id: msg.id ?? acc.id, model: msg.model ?? acc.model };
    }
    case "content_block_start": {
      const block = (d.content_block ?? {}) as { type?: string; id?: string; name?: string };
      if (block.type !== "tool_use") return acc;
      const next = cloneAssembly(acc);
      next.toolCalls.push({ id: block.id ?? "", type: "function", function: { name: block.name ?? "", arguments: "" } });
      next.blockIds = { ...blockIds, [blockIndex]: block.id ?? "" };
      return next;
    }
    case "content_block_delta": {
      const delta = (d.delta ?? {}) as { type?: string; text?: string; partial_json?: string };
      if (delta.type === "text_delta") return { ...cloneAssembly(acc), content: acc.content + (delta.text ?? "") };
      if (delta.type === "input_json_delta") {
        const next = cloneAssembly(acc);
        const id = blockIds[blockIndex];
        const call = next.toolCalls.find((t) => t.id === id) ?? next.toolCalls[next.toolCalls.length - 1];
        if (call) call.function.arguments += delta.partial_json ?? "";
        return next;
      }
      return acc;
    }
    case "message_delta": {
      const delta = (d.delta ?? {}) as { stop_reason?: string | null };
      const usage = mapUsage(d.usage as TokenUsage);
      return { ...cloneAssembly(acc), finishReason: delta.stop_reason ?? acc.finishReason, usage: usage ?? acc.usage };
    }
    case "error":
      return { ...cloneAssembly(acc), error: errorText(d) ?? "The stream reported an error." };
    default:
      return acc;
  }
}

/** Fold one stream event for `mode` (chat events are ChatCompletionChunks). */
export function mergeModeEvent(mode: PlaygroundMode, acc: ModeAssembly, event: string, data: unknown): ModeAssembly {
  if (mode === "responses") return mergeResponsesEvent(acc, event, data);
  if (mode === "messages") return mergeMessagesEvent(acc, event, data);
  return { ...acc, ...mergeChunk(acc, data as ChatCompletionChunk) };
}

/** Read a non-streamed body for `mode`. */
export function readModeReply(mode: PlaygroundMode, json: unknown): ModeAssembly {
  if (mode === "responses") return readResponseObject(json);
  if (mode === "messages") return readMessageObject(json);
  return { ...readCompletion(json), incompleteReason: null, error: null, raw: json };
}

/** The response JSON the inspector shows for a streamed call. */
export function modeAssemblyToJson(mode: PlaygroundMode, a: ModeAssembly): Record<string, unknown> {
  if (mode === "chat") return assembledToJson(a);
  if (mode === "responses" && a.raw && typeof a.raw === "object" && (a.raw as { object?: unknown }).object === "response") {
    return a.raw as Record<string, unknown>;
  }
  if (mode === "responses") {
    return {
      id: a.id,
      object: "response",
      model: a.model,
      status: a.finishReason,
      output: [
        ...(a.content ? [{ type: "message", role: "assistant", content: [{ type: "output_text", text: a.content }] }] : []),
        ...a.toolCalls.map((t) => ({ type: "function_call", call_id: t.id, name: t.function.name, arguments: t.function.arguments })),
      ],
      incomplete_details: a.incompleteReason ? { reason: a.incompleteReason } : null,
      usage: a.usage ? { input_tokens: a.usage.prompt_tokens, output_tokens: a.usage.completion_tokens } : undefined,
    };
  }
  return {
    id: a.id,
    type: "message",
    role: "assistant",
    model: a.model,
    content: [
      ...(a.content ? [{ type: "text", text: a.content }] : []),
      ...a.toolCalls.map((t) => ({ type: "tool_use", id: t.id, name: t.function.name, input: parseArgs(t.function.arguments) })),
    ],
    stop_reason: a.finishReason,
    usage: a.usage ? { input_tokens: a.usage.prompt_tokens, output_tokens: a.usage.completion_tokens } : undefined,
  };
}

// ─── Export for Responses and Messages ──────────────────────────────────────

/** curl / Python / TypeScript for `mode`; chat mode is exactly exportRequest. */
export function exportModeRequest(
  mode: PlaygroundMode,
  language: ExportLanguage,
  body: Record<string, unknown>,
  baseUrl: string
): string {
  if (mode === "chat") return exportRequest(language, body, baseUrl);
  const root = baseUrl.replace(/\/+$/, "");
  if (language === "curl") {
    return [
      `curl ${root}${MODE_PATHS[mode]} \\`,
      `  -H "Authorization: Bearer ${KEY_PLACEHOLDER}" \\`,
      `  -H "Content-Type: application/json" \\`,
      `  -d ${shellQuote(JSON.stringify(body, null, 2))}`,
    ].join("\n");
  }
  const { stream, ...args } = body as { stream?: boolean; [k: string]: unknown };
  const resource = mode === "responses" ? "responses" : "messages";
  if (language === "python") {
    const pyArgs = Object.entries(args)
      .map(([k, v]) => `    ${k}=${toPython(v, 4)},`)
      .join("\n");
    const streamLoop =
      mode === "responses"
        ? 'for event in stream:\n    if event.event == "response.output_text.delta":\n        print(event.data["delta"], end="")'
        : 'for event in stream:\n    if event.event == "content_block_delta" and event.data["delta"].get("type") == "text_delta":\n        print(event.data["delta"]["text"], end="")';
    return [
      "import os",
      "from allternit_platform import AllternitPlatform",
      "",
      "client = AllternitPlatform(",
      `    base_url="${root}",`,
      `    api_key=os.environ["ALLTERNIT_API_KEY"],  # ${KEY_PLACEHOLDER}`,
      ")",
      "",
      `${stream ? "stream" : "response"} = client.${resource}.${stream ? "create_stream" : "create"}(`,
      pyArgs,
      ")",
      "",
      stream ? streamLoop : mode === "responses" ? 'print(response["output"])' : 'print(response["content"])',
    ].join("\n");
  }
  const tsArgs = Object.entries(args)
    .map(([k, v]) => `  ${k}: ${indent(JSON.stringify(v, null, 2), 2)},`)
    .join("\n");
  const tsLoop =
    mode === "responses"
      ? 'for await (const event of stream) {\n  if (event.event === "response.output_text.delta") {\n    process.stdout.write((event.data as { delta: string }).delta);\n  }\n}'
      : 'for await (const event of stream) {\n  const delta = (event.data as { delta?: { type?: string; text?: string } }).delta;\n  if (event.event === "content_block_delta" && delta?.type === "text_delta") {\n    process.stdout.write(delta.text ?? "");\n  }\n}';
  return [
    'import { AllternitPlatform } from "@allternit/platform";',
    "",
    "const client = new AllternitPlatform({",
    `  baseUrl: "${root}",`,
    `  apiKey: process.env.ALLTERNIT_API_KEY, // ${KEY_PLACEHOLDER}`,
    "});",
    "",
    `const ${stream ? "stream" : "response"} = ${stream ? "" : "await "}client.${resource}.${stream ? "createStream" : "create"}({`,
    tsArgs,
    "});",
    "",
    stream ? tsLoop : mode === "responses" ? "console.log(response.output);" : "console.log(response.content);",
  ].join("\n");
}
