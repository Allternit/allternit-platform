/**
 * Live model catalog for console pickers.
 *
 * `GET /v1/models` returns flat rows on Allternit Cloud: id, name,
 * context_length, prompt_price and completion_price (USD per token),
 * supports_tools / supports_image_input (Fireworks rows) or
 * supported_parameters / architecture.input_modalities (OpenRouter rows).
 * Runtime gateways return the older shape with pricing.input_cents_per_1m.
 * Both are normalized into ConsoleModel. `pricing` (cents per 1M tokens) is
 * always derived so the auto-policy resolver keeps working unchanged.
 */

import { api } from "@/lib/api-client";
import { gatewayAuthOptions } from "@/lib/console-gateway";

export interface ConsoleModel {
  id: string;
  object?: string;
  created?: number;
  owned_by: string;
  display_name?: string;
  context_window?: number;
  quality_tier?: string;
  /** USD per token, when the row carries flat prices (Allternit Cloud). */
  prompt_price?: number;
  completion_price?: number;
  supports_tools?: boolean;
  supports_vision?: boolean;
  /** The model Cloud recommends (`default: true` on GET /v1/models). */
  default?: boolean;
  /** Derived cents per 1M tokens (auto policy + dashboard cards). */
  pricing?: {
    input_cents_per_1m?: number;
    output_cents_per_1m?: number;
  } | null;
}

interface ModelRowJson {
  id: string;
  owned_by?: string;
  display_name?: string;
  name?: string;
  quality_tier?: string;
  context_window?: number;
  context_length?: number;
  prompt_price?: number;
  completion_price?: number;
  supports_tools?: boolean;
  supports_image_input?: boolean;
  supported_parameters?: string[];
  architecture?: { input_modalities?: string[] };
  hidden?: boolean;
  default?: boolean;
  pricing?: {
    input_cents_per_1m?: number;
    output_cents_per_1m?: number;
  } | null;
}

export function normalizeModelRow(m: ModelRowJson): ConsoleModel {
  const params = Array.isArray(m.supported_parameters) ? m.supported_parameters : [];
  const modalities = Array.isArray(m.architecture?.input_modalities)
    ? m.architecture!.input_modalities!
    : [];
  const hasFlat = typeof m.prompt_price === "number" || typeof m.completion_price === "number";
  return {
    id: m.id,
    owned_by: m.owned_by ?? "unknown",
    display_name: m.display_name ?? m.name,
    context_window: m.context_window ?? m.context_length,
    quality_tier: m.quality_tier,
    prompt_price: m.prompt_price,
    completion_price: m.completion_price,
    supports_tools: m.supports_tools === true || params.includes("tools"),
    supports_vision: m.supports_image_input === true || modalities.includes("image"),
    ...(m.default === true ? { default: true } : {}),
    pricing: hasFlat
      ? {
          input_cents_per_1m: (m.prompt_price ?? 0) * 1e8,
          output_cents_per_1m: (m.completion_price ?? 0) * 1e8,
        }
      : (m.pricing ?? null),
  };
}

/**
 * `authKey` is the minted playground key on cloud; without it the call uses
 * the console session like before.
 */
export async function fetchConsoleModels(authKey?: string): Promise<ConsoleModel[]> {
  const data = await api.get<{ data?: ModelRowJson[] }>(
    "/v1/models",
    authKey ? gatewayAuthOptions(authKey) : undefined
  );
  return (data.data ?? [])
    .filter((m) => typeof m.id === "string" && m.id.length > 0 && !m.hidden)
    .map(normalizeModelRow);
}
