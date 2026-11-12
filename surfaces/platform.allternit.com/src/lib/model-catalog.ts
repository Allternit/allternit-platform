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
  /** Derived only from flags the API row carries: "Tools", "Vision", "JSON". */
  capabilities: string[];
  /** Formatted retirement date, only when the row has a non-null `expiration_date`. */
  retires?: string;
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
  /** Fireworks rows. */
  supports_tools?: boolean;
  supports_image_input?: boolean;
  /** OpenRouter rows. */
  supported_parameters?: string[];
  architecture?: { input_modalities?: string[] };
  /** Null on every row today; a date string once a model has a retirement date. */
  expiration_date?: string | null;
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

export function modelCapabilities(model: LiveModelInfo): string[] {
  const params = Array.isArray(model.supported_parameters) ? model.supported_parameters : [];
  const modalities = Array.isArray(model.architecture?.input_modalities)
    ? model.architecture!.input_modalities!
    : [];
  const out: string[] = [];
  if (model.supports_tools === true || params.includes("tools")) out.push("Tools");
  if (model.supports_image_input === true || modalities.includes("image")) out.push("Vision");
  if (params.includes("response_format") || params.includes("structured_outputs")) out.push("JSON");
  return out;
}

function retireDate(value: unknown): string | undefined {
  if (typeof value !== "string" || !value) return undefined;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return undefined;
  return date.toLocaleDateString("en-US", {
    year: "numeric",
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });
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
    capabilities: modelCapabilities(model),
    retires: retireDate(model.expiration_date),
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
