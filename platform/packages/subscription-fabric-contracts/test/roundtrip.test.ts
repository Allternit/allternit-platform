import { describe, expect, it } from "vitest";
import { z } from "zod";
import {
  accountSchema,
  adapterEventSchema,
  adapterManifestSchema,
  artifactSchema,
  capabilityDefSchema,
  quotaPoolSchema,
  routeDecisionSchema,
  taskInputSchema,
  taskSchema,
  threadMappingSchema,
} from "../src/index";
import {
  account,
  adapterEvents,
  adapterManifest,
  artifact,
  capabilityDef,
  quotaPool,
  routeDecision,
  task,
  threadMapping,
} from "./fixtures";

// Parse → JSON.stringify → parse must be deep-equal. This pins the schemas to
// JSON-serializable wire shapes (the gateway streams these over SSE).
function expectJsonRoundTrip<T extends z.ZodTypeAny>(
  schema: T,
  value: unknown
): z.infer<T> {
  const parsed = schema.parse(value);
  const reparsed = schema.parse(JSON.parse(JSON.stringify(parsed)));
  expect(reparsed).toEqual(parsed);
  return parsed;
}

describe("zod schema round-trips", () => {
  it("CapabilityDef", () => {
    expectJsonRoundTrip(capabilityDefSchema, capabilityDef);
  });

  it("AdapterManifest", () => {
    expectJsonRoundTrip(adapterManifestSchema, adapterManifest);
  });

  it("Account", () => {
    expectJsonRoundTrip(accountSchema, account);
  });

  it("QuotaPool", () => {
    expectJsonRoundTrip(quotaPoolSchema, quotaPool);
  });

  it("Task (all 5 input variants in one task)", () => {
    const parsed = expectJsonRoundTrip(taskSchema, task);
    expect(parsed.inputs.map((i) => i.type)).toEqual([
      "artifact",
      "file",
      "text",
      "url",
      "image",
    ]);
  });

  it.each([
    ["artifact", { type: "artifact", artifact_id: "artifact_example_00" }],
    [
      "file",
      {
        type: "file",
        path: "workspace/notes.txt",
        sha256: "e".repeat(64),
        size_bytes: 10,
      },
    ],
    ["text", { type: "text", name: "brief", content: "hi" }],
    ["url", { type: "url", url: "https://example.test/x" }],
    ["image", { type: "image", mime_type: "image/jpeg", data_base64: "AAAA" }],
  ])("TaskInput variant: %s", (_name, input) => {
    expectJsonRoundTrip(taskInputSchema, input);
  });

  it("Artifact", () => {
    expectJsonRoundTrip(artifactSchema, artifact);
  });

  it("ThreadMapping", () => {
    expectJsonRoundTrip(threadMappingSchema, threadMapping);
  });

  it("RouteDecision", () => {
    expectJsonRoundTrip(routeDecisionSchema, routeDecision);
  });

  it("every AdapterEvent variant", () => {
    for (const event of adapterEvents) {
      expectJsonRoundTrip(adapterEventSchema, event);
    }
  });

  it("artifact.partial carries an optional Uint8Array preview (non-JSON path)", () => {
    const event = {
      t: "artifact.partial",
      ref: {
        provider: "prov_example",
        provider_artifact_id: "provartifact_example_1",
        provider_url: "https://provider.example/artifacts/1",
        provider_url_expires_at: null,
      },
      preview: new Uint8Array([1, 2, 3]),
    };
    const parsed = adapterEventSchema.parse(event);
    expect(parsed.t).toBe("artifact.partial");
    expect((parsed as { preview?: Uint8Array }).preview).toBeInstanceOf(
      Uint8Array
    );
  });

  it("AdapterEvent covers all 12 variants", () => {
    expect(adapterEvents).toHaveLength(12);
  });
});
