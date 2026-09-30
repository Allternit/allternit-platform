// Vendor Packs: connection profiles, look profiles, PackGaps, channel packs.
import { z } from "zod";
import { guaranteeSchema, laneSchema } from "./agent";
import { authTypeSchema } from "./bindings";

export const connectionProfileSchema = z.object({
  id: z.string(),
  label: z.string(),
  authType: authTypeSchema,
  lane: laneSchema,
  guarantee: guaranteeSchema,
  recommended: z.boolean(),
  loginUrl: z.string().optional(),
  callback: z.string().optional(),
  loggedInProbe: z.string().optional(),
  sessionCookieHints: z.array(z.string()).optional(),
  scopes: z.array(z.string()),
  permissionDescription: z.array(z.string()),
  termsWarning: z.string().optional(),
  requiresUserOwnedSubscription: z.boolean().optional(),
});
export type ConnectionProfile = z.infer<typeof connectionProfileSchema>;

// SAFETY INVARIANT: the Allternit provenance layer (agent name, vendor +
// Verified, mode, lane, guarantee, status) is rendered above the pack surface
// and must not be hideable. This schema deliberately has NO field that could
// suppress, restyle or cover it. `viaBadge`, `verifiedBadge` and `laneBadge`
// only style the pack's own badges. Do not add such a field (z.object strips
// unknown keys, so extra input cannot smuggle one in).
export const lookProfileSchema = z.object({
  vendorId: z.string(),
  avatarTreatment: z.string(),
  accentTokens: z.record(z.string()),
  typographyHints: z.record(z.string()).optional(),
  iconAssets: z.record(z.string()).optional(),
  threadHeader: z.string().optional(),
  activityView: z.string().optional(),
  statusVocabulary: z.record(z.string()).optional(),
  approvalCard: z.string().optional(),
  computerView: z.string().optional(),
  taskCard: z.string().optional(),
  routineCard: z.string().optional(),
  composer: z.string().optional(),
  contentRenderers: z.array(z.string()),
  viaBadge: z.string().optional(),
  verifiedBadge: z.string().optional(),
  laneBadge: z.string().optional(),
});
export type LookProfile = z.infer<typeof lookProfileSchema>;

export const packGapSurfaceSchema = z.enum([
  "transcript", "composer", "activity", "card", "computer", "approval",
]);
export const packGapSeveritySchema = z.enum(["visual_parity", "functional", "data_loss"]);
export type PackGapSeverity = z.infer<typeof packGapSeveritySchema>;
export const packGapStatusSchema = z.enum(["open", "resolved", "wontfix"]);

export const packGapSchema = z.object({
  vendor: z.string(),
  capability: z.string(),
  surface: packGapSurfaceSchema,
  fallbackUsed: z.boolean(),
  severity: packGapSeveritySchema,
  status: packGapStatusSchema,
  firstSeenAt: z.string(),
  lastSeenAt: z.string(),
  occurrences: z.number().int().nonnegative(),
  sampleRef: z.string().optional(),
});
export type PackGap = z.infer<typeof packGapSchema>;

export type PackParity = "full" | "partial" | "blocked";

/**
 * full: zero open gaps. blocked: any open data_loss gap. partial: open gaps
 * exist but every one is visual_parity (owner acceptance is a separate
 * decision). Open functional gaps also fail full parity and cannot be
 * accepted as partial, so they block.
 */
export function packParity(gaps: readonly Pick<PackGap, "severity" | "status">[]): PackParity {
  const open = gaps.filter((g) => g.status === "open");
  if (open.length === 0) return "full";
  if (open.every((g) => g.severity === "visual_parity")) return "partial";
  return "blocked";
}

export const vendorPackManifestSchema = z.object({
  id: z.string(),
  version: z.string(),
  vendor: z.object({
    id: z.string(),
    displayName: z.string(),
    logoAsset: z.string(),
    brandProfile: z.record(z.unknown()),
    legalName: z.string().optional(),
  }),
  modes: z.array(z.enum(["native", "hosted", "linked", "mirror"])),
  adapters: z.array(z.string()),
  recommendedLane: laneSchema,
  connectionProfiles: z.array(connectionProfileSchema),
  capabilityProfile: z.record(z.unknown()),
  lookProfile: lookProfileSchema,
  terminology: z.record(z.string()),
  discoveryProfile: z.record(z.unknown()),
  approvalProfile: z.record(z.unknown()),
  computerProfile: z.record(z.unknown()),
  threadProfile: z.record(z.unknown()),
  consentProfile: z.record(z.unknown()),
  termsProfile: z.record(z.unknown()),
  verificationProfile: z.record(z.unknown()),
});
export type VendorPackManifest = z.infer<typeof vendorPackManifestSchema>;

export const channelPackManifestSchema = z.object({
  id: z.string(),
  version: z.string(),
  provider: z.string(),
  displayName: z.string(),
  logoAsset: z.string(),
  connectionProfiles: z.array(connectionProfileSchema),
  supportsBidirectional: z.boolean(),
  postingIdentity: z.enum(["channel_app", "exact_identity", "none"]),
  events: z.array(
    z.enum([
      "channel.message.received",
      "channel.message.sent",
      "channel.reaction.updated",
      "channel.message.edited",
      "channel.message.deleted",
    ]),
  ),
  cardRenderers: z.array(z.string()),
  termsProfile: z.record(z.unknown()).optional(),
});
export type ChannelPackManifest = z.infer<typeof channelPackManifestSchema>;
