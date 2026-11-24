import { api } from "@/lib/api-client";

export interface MeterPrice {
  meter: string;
  name?: string;
  unit: string;
  /** Cents per unit, decimal string. */
  payg_cents: string;
  growth_cents: string;
}

export interface ToolPrice {
  tool: string;
  unit: string;
  price_usd: number;
  provider: string;
}

export interface Deprecation {
  model: string;
  retires_at: string;
  replacement?: string | null;
}

export interface Pricing {
  meters: MeterPrice[];
  tools: ToolPrice[];
  deprecations: Deprecation[];
}

export async function getPricing(): Promise<Pricing> {
  const p = await api.get<Partial<Pricing>>("/api/v1/platform/pricing");
  return { meters: p.meters ?? [], tools: p.tools ?? [], deprecations: p.deprecations ?? [] };
}

/** Format a cents-per-unit decimal string as dollars, keeping small prices exact. */
export function formatCents(cents: string): string {
  const n = Number(cents);
  if (!Number.isFinite(n)) return "—";
  const usd = n / 100;
  return `$${usd >= 0.01 ? usd.toFixed(2) : usd.toPrecision(2).replace(/\.?0+$/, "")}`;
}
