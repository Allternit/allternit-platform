// §S2 — adapter manifest; PacingProfile per §A5
import { z } from "zod";
import { agentCapabilityManifestSchema } from "./agent";
import { capabilityIdSchema, providerIdSchema } from "./capability";
import { connectionProfileSchema } from "./vendor-pack";

export const planDefSchema = z.object({
  plan_id: z.string(),
  label: z.string(),
  notes: z.string().optional(),
});
export type PlanDef = z.infer<typeof planDefSchema>;

export const pacingProfileSchema = z.object({
  min_action_gap_ms: z.tuple([z.number(), z.number()]),
  min_task_gap_s: z.number(),
  max_tasks_per_hour: z.number(),
  max_tasks_per_day: z.number(),
  quiet_hours: z.tuple([z.number(), z.number()]).optional(),
});
export type PacingProfile = z.infer<typeof pacingProfileSchema>;

export const manifestCapabilitySchema = z.object({
  id: capabilityIdSchema,
  min_capability_version: z.number().int(),
  plans: z.array(z.string()),
  pool_id: z.string(),
  detachable: z.boolean(),
  export_formats: z.array(z.string()),
  max_inputs: z
    .object({
      files: z.number().int(),
      bytes_per_file: z.number().int(),
    })
    .optional(),
  status: z.enum(["stable", "beta", "disabled"]),
});
export type ManifestCapability = z.infer<typeof manifestCapabilitySchema>;

// Optional Agent Gateway section (AAI vNext). Absent on legacy manifests.
export const adapterAgentSectionSchema = z.object({
  capabilities: agentCapabilityManifestSchema,
  // Generalized auth descriptors, one per supported ConnectionProfile authType.
  authDescriptors: z.array(connectionProfileSchema),
});
export type AdapterAgentSection = z.infer<typeof adapterAgentSectionSchema>;

export const adapterManifestSchema = z.object({
  adapter_id: z.string(),
  adapter_version: z.string(),
  provider: providerIdSchema,
  interface: z.enum(["official", "ui_bridge_web", "ui_bridge_desktop"]),
  origins: z.array(z.string()),
  auth: z.object({
    login_url: z.string(),
    logged_in_probe: z.string(),
    // Cookie names (prefixes) the provider sets only once signed in: the
    // gateway watches the login browser for them to finish a login by itself.
    session_cookies: z.array(z.string()).optional(),
    // Same, for providers that keep the session in localStorage instead
    // (Kimi: access_token on its app origin).
    session_storage: z.array(z.object({ origin: z.string(), key: z.string() })).optional(),
  }),
  plans: z.array(planDefSchema),
  capabilities: z.array(manifestCapabilitySchema),
  pacing: pacingProfileSchema,
  selectors_version: z.string(),
  agent: adapterAgentSectionSchema.optional(),
});
export type AdapterManifest = z.infer<typeof adapterManifestSchema>;
