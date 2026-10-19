// §S3 — Account; SessionHealth union per HARDENING.md
import { z } from "zod";
import { providerIdSchema } from "./capability";

export const sessionHealthSchema = z.enum([
  "ready",
  "degraded",
  "auth_required",
  "challenge_presented",
  "account_restricted",
  "ui_drift",
  "provider_down",
  "profile_locked",
]);
export type SessionHealth = z.infer<typeof sessionHealthSchema>;

// What the account's own page shows about usage (e.g. ChatGPT's "8% usage
// remaining"); every field is what was observed, never estimated.
// One usage limit as the vendor's own usage page shows it (claude.ai: "5-hour",
// "Weekly", "Weekly (Opus)"; ChatGPT: "Usage"; Kimi: "Plan").
export const accountUsageWindowSchema = z.object({
  id: z.string(),
  label: z.string(),
  remaining_pct: z.number().nullable(),
  resets_at: z.string().nullable(),
});
export type AccountUsageWindow = z.infer<typeof accountUsageWindowSchema>;

// remaining_pct/resets_at stay the most-constrained window for older clients;
// `windows` carries every limit the vendor shows, in the vendor's order.
export const accountUsageSchema = z.object({
  remaining_pct: z.number().nullable(),
  resets_at: z.string().nullable(),
  observed_at: z.string(),
  windows: z.array(accountUsageWindowSchema).optional(),
});
export type AccountUsage = z.infer<typeof accountUsageSchema>;

export const accountSchema = z.object({
  account_id: z.string(),
  provider: providerIdSchema,
  label: z.string(),
  plan: z.string().nullable(),
  plan_observed_at: z.string().nullable(),
  profile_ref: z.string(),
  session_health: sessionHealthSchema,
  enabled: z.boolean(),
  // Read from the provider after sign-in (email/username). Never a token.
  identity: z.string().nullable().optional(),
  usage: accountUsageSchema.nullable().optional(),
  // Agents the account has on the provider (e.g. Claude Projects), as read from
  // the provider after sign-in. Public identity only; never credentials.
  agents: z.array(z.object({ id: z.string(), name: z.string(), kind: z.string().optional(), kindLabel: z.string().optional(), avatarUrl: z.string().optional() })).nullable().optional(),
  // The account the router tries first for its provider (one per provider).
  preferred: z.boolean().optional(),
});
export type Account = z.infer<typeof accountSchema>;
