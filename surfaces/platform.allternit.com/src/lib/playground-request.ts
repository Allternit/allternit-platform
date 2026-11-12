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
