/**
 * Live model catalog for console pickers.
 *
 * `GET /v1/models` is served by the Fabric model gateway
 * (`cmd/allternit-api/src/fabric_model_routes.rs`) and authenticates with the
 * Clerk session (unlike the virtual-key `/v1/*` routes). Model ids are full
 * ids in the form Allternit Cloud lists, e.g. `kimi-k3`.
 *
 * The shape intentionally mirrors the ai surface's ModelGatewayModel so the
 * auto-policy resolver (`lib/model-auto-policy.ts`, ported from the model
 * gateway view) can consume it unchanged.
 */

import { api } from "@/lib/api-client";

export interface ConsoleModel {
  id: string;
  object?: string;
  created?: number;
  owned_by: string;
  display_name?: string;
  context_window?: number;
  quality_tier?: string;
  pricing?: {
    input_cents_per_1m?: number;
    output_cents_per_1m?: number;
  } | null;
}

interface FabricModelJson {
  id: string;
  owned_by?: string;
  display_name?: string;
  quality_tier?: string;
  context_window?: number;
  pricing?: {
    input_cents_per_1m?: number;
    output_cents_per_1m?: number;
  } | null;
}

export async function fetchConsoleModels(): Promise<ConsoleModel[]> {
  const data = await api.get<{ data?: FabricModelJson[] }>("/v1/models");
  return (data.data ?? [])
    .filter((m) => typeof m.id === "string" && m.id.length > 0)
    .map((m) => ({ ...m, owned_by: m.owned_by ?? "unknown" }));
}
