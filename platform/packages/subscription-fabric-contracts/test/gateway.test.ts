import { describe, expect, it } from "vitest";
import { z } from "zod";
import {
  CONNECTION_TRANSITIONS,
  EXECUTION_TRANSITIONS,
  REMOTE_THREAD_TRANSITIONS,
  aaiErrorSchema,
  aaiOperationSchema,
  adapterManifestSchema,
  agentCapabilityManifestSchema,
  approvalSchema,
  botExecutionBindingSchema,
  canTransitionConnection,
  canTransitionExecution,
  canTransitionRemoteThread,
  channelConversationBindingSchema,
  channelPackManifestSchema,
  connectionProfileSchema,
  gatewayEventSchema,
  lookProfileSchema,
  memoryRecordSchema,
  mirrorFieldStateSchema,
  packGapSchema,
  packParity,
  providerAccountBindingSchema,
  remoteThreadBindingSchema,
  threadOriginSchema,
  vendorPackManifestSchema,
} from "../src/index";
import type { PackGap } from "../src/index";
import { adapterManifest } from "./fixtures";

function rt<T extends z.ZodTypeAny>(schema: T, v: unknown) {
  const parsed = schema.parse(v);
  expect(schema.parse(JSON.parse(JSON.stringify(parsed)))).toEqual(parsed);
  return parsed;
}

const t = (b: boolean) => b;
const caps = {
  vendor: "openai", adapterId: "openai-web", lane: "ui_bridge", guarantee: "best_effort",
  context: { supported: t(true), resume: t(true), parallel: t(false), maxParallel: 1, isolation: "isolated" },
  messaging: { send: true, stream: true, steer: false, interrupt: false, cancel: true },
  memory: { read: false, write: false, snapshot: false, opaque: true },
  tools: { tools: true, mcp: false, plugins: false, connectors: false },
  tasks: { list: true, schedule: false, cancel: false, background: false },
  approvals: { read: true, respond: true, exact: false },
  computer: { view: false, control: false, takeover: false },
  artifacts: { read: true, write: false, export: false },
  events: { native: false, polling: true, transcriptDerived: true, replay: false },
  runtime: { alwaysOn: false, localRequired: true, cloud: false },
};
const profile = {
  id: "cp1", label: "Browser sign-in", authType: "browser_session", lane: "ui_bridge",
  guarantee: "best_effort", recommended: true, loginUrl: "https://x.example/login",
  loggedInProbe: "composer", scopes: [], permissionDescription: ["read chats"],
};
const look = {
  vendorId: "openai", avatarTreatment: "dot", accentTokens: { accent: "#000" }, contentRenderers: ["dot.card"],
};
const gap = (severity: PackGap["severity"], status: PackGap["status"]): PackGap => ({
  vendor: "openai", capability: "dot.card.foo", surface: "card", fallbackUsed: true,
  severity, status, firstSeenAt: "2026-09-29T00:00:00Z", lastSeenAt: "2026-09-29T00:00:00Z", occurrences: 1,
});

describe("AAI schemas", () => {
  it("roundtrips capability manifest, error, event, approval, origin, memory, mirror", () => {
    rt(agentCapabilityManifestSchema, caps);
    rt(aaiErrorSchema, { code: "RATE_LIMITED", retryable: true, retryAfterMs: 1000, humanMessage: "slow down" });
    rt(gatewayEventSchema, {
      type: "agent.message.delta", botId: "b", threadId: "t", generationId: "g", source: "vendor",
      causationId: "c1", correlationId: "c2", guarantee: "inferred", lane: "ui_bridge",
    });
    rt(approvalSchema, { authority: "vendor", actor: "openai", action: "send", threadId: "t", remoteRef: "r", state: "pending" });
    rt(threadOriginSchema, { type: "channel", provider: "slack", externalConversationId: "C1", externalMessageId: "m1" });
    rt(memoryRecordSchema, { scope: "bot", source: "vendor", authority: "vendor", promotable: false });
    rt(mirrorFieldStateSchema, { field: "instructions", authority: "vendor", observability: "partial", status: "conflict" });
    expect(mirrorFieldStateSchema.safeParse({ field: "x", observability: "exact", status: "in_sync" }).success).toBe(false);
  });
  it("lists all 18 operations incl. agent.context.*", () => {
    expect(aaiOperationSchema.options).toHaveLength(18);
    expect(aaiOperationSchema.options).toContain("agent.context.steer");
  });
  it("rejects bad values", () => {
    expect(aaiErrorSchema.safeParse({ code: "NOPE", retryable: false, humanMessage: "x" }).success).toBe(false);
    expect(gatewayEventSchema.safeParse({ type: "agent.bogus" }).success).toBe(false);
    expect(approvalSchema.safeParse({ authority: "other", actor: "a", action: "b", threadId: "t", state: "pending" }).success).toBe(false);
    expect(agentCapabilityManifestSchema.safeParse({ ...caps, lane: "carrier_pigeon" }).success).toBe(false);
    expect(agentCapabilityManifestSchema.safeParse({ ...caps, context: { ...caps.context, isolation: "x" } }).success).toBe(false);
  });
});

describe("bindings", () => {
  it("roundtrips wire types", () => {
    rt(botExecutionBindingSchema, { id: "1", botId: "b", type: "vendor", mode: "linked", vendor: "openai", state: "READY" });
    rt(providerAccountBindingSchema, { id: "a", owner: "u", vendor: "openai", authType: "oauth", scopes: ["r"], state: "CONNECTED" });
    rt(remoteThreadBindingSchema, { id: "r", threadId: "t", generation: 1, executionBindingId: "1", lane: "official", state: "ACTIVE" });
    rt(channelConversationBindingSchema, {
      threadId: "t", provider: "slack", accountBindingId: "a", externalConversationId: "C1",
      bidirectional: true, readOnly: false, syncState: "LIVE",
    });
  });
  it("rejects bad states", () => {
    expect(botExecutionBindingSchema.safeParse({ id: "1", botId: "b", type: "vendor", mode: "linked", state: "OK" }).success).toBe(false);
    expect(providerAccountBindingSchema.safeParse({ id: "a", owner: "u", vendor: "v", authType: "password", scopes: [], state: "CONNECTED" }).success).toBe(false);
  });
  it("connection transitions", () => {
    expect(canTransitionConnection("DISCONNECTED", "CONSENT_REQUIRED")).toBe(true);
    expect(canTransitionConnection("VERIFYING", "CONNECTED")).toBe(true);
    expect(canTransitionConnection("CONNECTED", "BLOCKED")).toBe(true);
    expect(canTransitionConnection("DISCONNECTED", "CONNECTED")).toBe(false);
    expect(canTransitionConnection("CONNECTED", "AUTHENTICATING")).toBe(false);
    expect(Object.keys(CONNECTION_TRANSITIONS)).toHaveLength(10);
  });
  it("execution transitions", () => {
    expect(canTransitionExecution("UNBOUND", "BOUND")).toBe(true);
    expect(canTransitionExecution("BOUND", "READY")).toBe(true);
    expect(canTransitionExecution("UNBOUND", "READY")).toBe(false);
    expect(canTransitionExecution("FAILED", "READY")).toBe(false);
    expect(canTransitionExecution("FAILED", "BOUND")).toBe(true);
    expect(Object.keys(EXECUTION_TRANSITIONS)).toHaveLength(8);
  });
  it("remote thread transitions", () => {
    expect(canTransitionRemoteThread("UNBOUND", "OPENING")).toBe(true);
    expect(canTransitionRemoteThread("OPENING", "ACTIVE")).toBe(true);
    expect(canTransitionRemoteThread("OPENING", "UNBOUND")).toBe(true); // failed open, retried next turn
    expect(canTransitionRemoteThread("ACTIVE", "HANDOFF_PENDING")).toBe(true);
    expect(canTransitionRemoteThread("CLOSED", "ACTIVE")).toBe(false);
    expect(canTransitionRemoteThread("UNBOUND", "ACTIVE")).toBe(false);
    expect(REMOTE_THREAD_TRANSITIONS.CLOSED).toEqual([]);
  });
});

describe("vendor packs", () => {
  it("roundtrips profiles and manifests", () => {
    rt(connectionProfileSchema, profile);
    rt(lookProfileSchema, look);
    rt(packGapSchema, gap("visual_parity", "open"));
    const o = {};
    rt(vendorPackManifestSchema, {
      id: "openai", version: "1", vendor: { id: "openai", displayName: "OpenAI", logoAsset: "l.svg", brandProfile: o },
      modes: ["linked"], adapters: ["openai-web"], recommendedLane: "ui_bridge", connectionProfiles: [profile],
      capabilityProfile: o, lookProfile: look, terminology: { thread: "chat" }, discoveryProfile: o,
      approvalProfile: o, computerProfile: o, threadProfile: o, consentProfile: o, termsProfile: o, verificationProfile: o,
    });
    rt(channelPackManifestSchema, {
      id: "slack", version: "1", provider: "slack", displayName: "Slack", logoAsset: "s.svg",
      connectionProfiles: [{ ...profile, authType: "channel_oauth", lane: "channel" }],
      supportsBidirectional: true, postingIdentity: "channel_app", events: ["channel.message.received"], cardRenderers: [],
    });
  });
  it("look profile has no provenance-suppression field and strips smuggled ones", () => {
    const keys = Object.keys(lookProfileSchema.shape).join(" ").toLowerCase();
    expect(keys).not.toMatch(/hide|suppress|provenance|override/);
    const parsed = lookProfileSchema.parse({ ...look, hideProvenance: true });
    expect(parsed).not.toHaveProperty("hideProvenance");
  });
  it("rejects bad pack data", () => {
    expect(connectionProfileSchema.safeParse({ ...profile, authType: "password" }).success).toBe(false);
    expect(packGapSchema.safeParse({ ...gap("functional", "open"), severity: "cosmetic" }).success).toBe(false);
    expect(packGapSchema.safeParse({ ...gap("functional", "open"), status: "closed" }).success).toBe(false);
  });
  it("packParity", () => {
    expect(packParity([])).toBe("full");
    expect(packParity([gap("data_loss", "resolved"), gap("functional", "wontfix")])).toBe("full");
    expect(packParity([gap("visual_parity", "open"), gap("functional", "resolved")])).toBe("partial");
    expect(packParity([gap("visual_parity", "open"), gap("data_loss", "open")])).toBe("blocked");
    expect(packParity([gap("functional", "open")])).toBe("blocked");
  });
});

describe("AdapterManifest agent extension", () => {
  it("legacy manifest without agent still parses", () => {
    const parsed = adapterManifestSchema.parse(adapterManifest);
    expect(parsed.agent).toBeUndefined();
  });
  it("accepts and roundtrips an agent section; rejects a malformed one", () => {
    const withAgent = { ...adapterManifest, agent: { capabilities: caps, authDescriptors: [profile] } };
    rt(adapterManifestSchema, withAgent);
    expect(adapterManifestSchema.safeParse({ ...adapterManifest, agent: { capabilities: {}, authDescriptors: [] } }).success).toBe(false);
  });
});
