export interface CatalogModel {
  id: string;
  name: string;
  family: string;
  provider: "local" | "cloud";
  upstreamProvider?: string;
  upstreamId?: string;
  inputPrice: string;
  outputPrice: string;
  context: string;
}

export interface LiveModelInfo {
  id: string;
  object: string;
  created: number;
  owned_by: string;
  upstream_id?: string;
  provider?: string;
  aliases?: string[];
  name?: string;
  prompt_price?: number;
  completion_price?: number;
  context_length?: number;
  /** "standard" or "open" (open-weight models with far fewer refusals). */
  lane?: string;
  extra?: Record<string, unknown>;
}

export interface LiveModelListResponse {
  object: string;
  data: LiveModelInfo[];
}

function cloudApiBase(): string {
  return String(
    import.meta.env.VITE_ALLTERNIT_CLOUD_API_URL || "https://api.allternit.com",
  ).replace(/\/$/, "");
}

function modelFamily(id: string): string {
  const lower = id.toLowerCase();
  if (lower.includes("llama")) return "Llama";
  if (lower.includes("qwen")) return "Qwen";
  if (lower.includes("deepseek")) return "DeepSeek";
  if (lower.includes("kimi")) return "Kimi";
  if (lower.includes("glm")) return "GLM";
  if (lower.includes("mistral")) return "Mistral";
  if (lower.includes("mixtral")) return "Mixtral";
  if (lower.includes("claude")) return "Claude";
  if (lower.includes("gpt")) return "GPT";
  if (lower.includes("gemini")) return "Gemini";
  return "Other";
}

/**
 * Normalize upstream price to a per-1M-token display value.
 *
 * Some providers (e.g. OpenRouter) return per-token prices (very small
 * floats), while others (Together, Fireworks, DeepInfra) return per-1M-token
 * prices. We treat values below $0.001 as per-token and multiply by 1M.
 */
function formatPrice(value: unknown): string {
  if (typeof value === "number") {
    if (value === 0) return "$0";
    const perMillion = value < 0.001 ? value * 1_000_000 : value;
    if (perMillion < 0.01) return `$${perMillion.toFixed(4)}`;
    return `$${perMillion.toFixed(2)}`;
  }
  return "—";
}

function contextLength(model: LiveModelInfo): string {
  const ctx = model.context_length ?? model.extra?.context_length;
  if (typeof ctx === "number") {
    if (ctx >= 1_000_000) return `${(ctx / 1_000_000).toFixed(1)}M`;
    if (ctx >= 1000) return `${(ctx / 1000).toFixed(0)}K`;
    return String(ctx);
  }
  return "—";
}

export function liveModelToCatalog(model: LiveModelInfo): CatalogModel {
  const upstreamProvider = model.provider || model.owned_by || "cloud";
  return {
    id: model.id,
    name: String(model.name ?? model.extra?.name ?? model.id),
    family: modelFamily(model.id),
    provider: upstreamProvider === "local" ? "local" : "cloud",
    upstreamProvider,
    upstreamId: model.upstream_id,
    inputPrice: formatPrice(model.prompt_price ?? model.extra?.prompt_price),
    outputPrice: formatPrice(model.completion_price ?? model.extra?.completion_price),
    context: contextLength(model),
  };
}

export async function fetchLiveModelCatalog(
  signal?: AbortSignal,
): Promise<LiveModelListResponse> {
  const response = await fetch(`${cloudApiBase()}/v1/models`, {
    headers: { Accept: "application/json" },
    signal,
  });
  if (!response.ok) {
    const payload = await response.json().catch(() => ({}));
    throw new Error(
      payload.message ||
        payload.error ||
        `Model catalog request failed (${response.status})`,
    );
  }
  return response.json();
}

export const DRAFT_MODEL_CATALOG: CatalogModel[] = [
  { id: "llama-3.1-8b", name: "Llama 3.1 8B", family: "Llama", provider: "local", inputPrice: "$0", outputPrice: "$0", context: "128K" },
  { id: "llama-3.1-70b", name: "Llama 3.1 70B", family: "Llama", provider: "local", inputPrice: "$0", outputPrice: "$0", context: "128K" },
  { id: "qwen2.5-7b", name: "Qwen 2.5 7B", family: "Qwen", provider: "local", inputPrice: "$0", outputPrice: "$0", context: "128K" },
  { id: "qwen2.5-72b", name: "Qwen 2.5 72B", family: "Qwen", provider: "local", inputPrice: "$0", outputPrice: "$0", context: "128K" },
  { id: "mistral-nemo", name: "Mistral Nemo", family: "Mistral", provider: "local", inputPrice: "$0", outputPrice: "$0", context: "128K" },
  { id: "mistral-large", name: "Mistral Large 2", family: "Mistral", provider: "cloud", inputPrice: "$2.00", outputPrice: "$6.00", context: "128K" },
  { id: "claude-3-5-sonnet", name: "Claude 3.5 Sonnet", family: "Claude", provider: "cloud", inputPrice: "$3.00", outputPrice: "$15.00", context: "200K" },
  { id: "claude-3-opus", name: "Claude 3 Opus", family: "Claude", provider: "cloud", inputPrice: "$15.00", outputPrice: "$75.00", context: "200K" },
  { id: "gpt-4o", name: "GPT-4o", family: "GPT", provider: "cloud", inputPrice: "$5.00", outputPrice: "$15.00", context: "128K" },
  { id: "gpt-4o-mini", name: "GPT-4o mini", family: "GPT", provider: "cloud", inputPrice: "$0.15", outputPrice: "$0.60", context: "128K" },
  { id: "gemini-1.5-pro", name: "Gemini 1.5 Pro", family: "Gemini", provider: "cloud", inputPrice: "$3.50", outputPrice: "$10.50", context: "1M" },
  { id: "gemini-1.5-flash", name: "Gemini 1.5 Flash", family: "Gemini", provider: "cloud", inputPrice: "$0.35", outputPrice: "$1.05", context: "1M" },
];
