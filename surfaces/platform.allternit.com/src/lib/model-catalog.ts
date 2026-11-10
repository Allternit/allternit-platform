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
  /** The recommended Cloud model (`default: true` in `GET /v1/models`). */
  isDefault?: boolean;
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
  /** True on the one recommended model; the API lists it first. */
  default?: boolean;
  /** Still routes (saved ids keep working) but is not offered in pickers. */
  hidden?: boolean;
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
    isDefault: (model.default ?? model.extra?.default) === true,
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
  const body = (await response.json()) as LiveModelListResponse;
  // The console never shows hidden rows (stale or duplicate aliases).
  return { ...body, data: (body.data ?? []).filter((model) => !isHidden(model)) };
}

/** A `GET /v1/models` row flagged `hidden: true`. */
export function isHidden(model: LiveModelInfo): boolean {
  return (model.hidden ?? model.extra?.hidden) === true;
}

// Fallback when the live list is unreachable: the listed (non-hidden) rows of
// `GET /v1/models` as of 2026-11-10, default (kimi-k3) first, then catalog
// order. Prices are what a request is billed (provider price × the default
// 1.5 markup), formatted the way `liveModelToCatalog` formats the live list.
// Keep in step with cmd/allternit-cloud-api/src/model_router/catalog.rs.
export const DRAFT_MODEL_CATALOG: CatalogModel[] = [
  { id: "kimi-k3", name: "Kimi K3", family: "Kimi", provider: "cloud", upstreamProvider: "fireworks", inputPrice: "$4.50", outputPrice: "$22.50", context: "1.0M", isDefault: true },
  { id: "claude-sonnet-5.5", name: "Claude Sonnet 5.5", family: "Claude", provider: "cloud", upstreamProvider: "openrouter", inputPrice: "$3.00", outputPrice: "$15.00", context: "1.0M" },
  { id: "gpt-6.1-sol", name: "GPT-6.1 Sol", family: "GPT", provider: "cloud", upstreamProvider: "openrouter", inputPrice: "$3.00", outputPrice: "$15.00", context: "1.1M" },
  { id: "deepseek-v4.1-flash", name: "DeepSeek V4.1 Flash", family: "DeepSeek", provider: "cloud", upstreamProvider: "openrouter", inputPrice: "$0.45", outputPrice: "$1.80", context: "1.0M" },
  { id: "glm-5.3-flash", name: "GLM 5.3 Flash", family: "GLM", provider: "cloud", upstreamProvider: "openrouter", inputPrice: "$0.06", outputPrice: "$0.12", context: "1.0M" },
  { id: "deepseek-v4-pro", name: "DeepSeek V4 Pro", family: "DeepSeek", provider: "cloud", upstreamProvider: "together", inputPrice: "$1.98", outputPrice: "$5.94", context: "1.0M" },
  { id: "glm-5.3", name: "GLM 5.3", family: "GLM", provider: "cloud", upstreamProvider: "together", inputPrice: "$2.10", outputPrice: "$6.60", context: "1.0M" },
  { id: "kimi-k3.1-pro", name: "Kimi K3.1 Pro", family: "Kimi", provider: "cloud", upstreamProvider: "together", inputPrice: "$3.00", outputPrice: "$15.00", context: "1.0M" },
  { id: "qwen3.8-max-fireworks", name: "Qwen 3.8 Max (Fireworks)", family: "Qwen", provider: "cloud", upstreamProvider: "fireworks", inputPrice: "$3.00", outputPrice: "$9.00", context: "1.0M" },
];
