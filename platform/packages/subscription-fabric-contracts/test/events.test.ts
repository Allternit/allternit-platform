import { describe, expect, it } from "vitest";
import type {
  AdapterEvent,
  ExecutionContext,
  ProbeResult,
  ReconcileResult,
  ResumeToken,
  SubscriptionAdapter,
  Task,
  TaskInput,
} from "../src/index";
import {
  adapterEventSchema,
  probeResultSchema,
  reconcileResultSchema,
  resumeTokenSchema,
} from "../src/index";
import { adapterEvents, adapterManifest, task } from "./fixtures";

function assertNever(value: never): never {
  throw new Error(`unhandled variant: ${JSON.stringify(value)}`);
}

// --- Exhaustiveness over the AdapterEvent union -----------------------------
// The `default: assertNever(e)` arm makes a newly-added (or removed) variant a
// compile error, not a silent pass.
function classify(event: AdapterEvent): string {
  switch (event.t) {
    case "submitted":
      return `submitted:${String(event.provider_thread_id)}`;
    case "reply":
      return `reply:${event.event.type}`;
    case "progress":
      return `progress:${event.label}`;
    case "progress.heartbeat":
      return `progress.heartbeat:${event.elapsed_s}`;
    case "artifact.partial":
      return `artifact.partial:${event.ref.provider_artifact_id}`;
    case "artifact.ready":
      return `artifact.ready:${event.ref.provider_artifact_id}`;
    case "model.observed":
      return `model.observed:${event.model}`;
    case "quota.signal":
      return `quota.signal:${event.signal.kind}`;
    case "needs_user":
      return `needs_user:${event.reason}`;
    case "detached":
      return `detached:${event.resume_token.token}`;
    case "done":
      return `done:${event.outcome}`;
    case "error":
      return `error:${event.error.class}`;
    default:
      return assertNever(event);
  }
}

// --- Exhaustiveness over the TaskInput union --------------------------------
function classifyInput(input: TaskInput): string {
  switch (input.type) {
    case "artifact":
      return `artifact:${input.artifact_id}`;
    case "file":
      return `file:${input.path}`;
    case "text":
      return `text:${input.name}`;
    case "url":
      return `url:${input.url}`;
    case "image":
      return `image:${input.mime_type}`;
    default:
      return assertNever(input);
  }
}

describe("AdapterEvent union", () => {
  it("every variant parses and classifies without falling through", () => {
    for (const event of adapterEvents) {
      const parsed = adapterEventSchema.parse(event);
      expect(classify(parsed)).not.toBe("");
    }
  });

  it("all 12 tags are covered exactly once", () => {
    const tags = adapterEvents.map((e) => e.t);
    expect(new Set(tags).size).toBe(12);
  });
});

describe("Task/ TaskInput union", () => {
  it("every input variant classifies without falling through", () => {
    const parsed: Task = task;
    for (const input of parsed.inputs) {
      expect(classifyInput(input)).not.toBe("");
    }
  });
});

describe("ProbeResult / ResumeToken / ReconcileResult round-trips", () => {
  const probe: ProbeResult = {
    ok: false,
    checks: [
      { key: "composer", critical: true, ok: true },
      { key: "stop_button", critical: true, ok: false, detail: "not found" },
    ],
    observed_at: "2026-01-01T00:00:00.000Z",
  };

  const token: ResumeToken = {
    token: "opaque",
    adapter_id: "adapt_example_web",
    attempt_no: 2,
    issued_at: "2026-01-01T00:00:00.000Z",
    poll_after_s: 45,
  };

  const reconcile: ReconcileResult = {
    outcome: "acknowledged",
    provider_thread_id: "provthread_example_1",
    detail: "adopted existing user turn",
  };

  it("ProbeResult", () => {
    const parsed = probeResultSchema.parse(probe);
    expect(probeResultSchema.parse(JSON.parse(JSON.stringify(parsed)))).toEqual(
      parsed
    );
  });

  it("ResumeToken", () => {
    const parsed = resumeTokenSchema.parse(token);
    expect(resumeTokenSchema.parse(JSON.parse(JSON.stringify(parsed)))).toEqual(
      parsed
    );
  });

  it("ReconcileResult", () => {
    const parsed = reconcileResultSchema.parse(reconcile);
    expect(
      reconcileResultSchema.parse(JSON.parse(JSON.stringify(parsed)))
    ).toEqual(parsed);
  });

  it("ReconcileResult with only an outcome is valid (optionals are optional)", () => {
    expect(reconcileResultSchema.parse({ outcome: "not_found" })).toEqual({
      outcome: "not_found",
    });
  });
});

describe("SubscriptionAdapter boundary contract is implementable", () => {
  it("a minimal adapter satisfies the interface shape", () => {
    const adapter: SubscriptionAdapter = {
      manifest: adapterManifest,
      async attach() {},
      async detach() {},
      async probe() {
        return { ok: true, checks: [], observed_at: "2026-01-01T00:00:00.000Z" };
      },
      async *execute(_task: Task, _ctx: ExecutionContext): AsyncIterable<AdapterEvent> {
        yield { t: "done", outcome: "success" };
      },
    };
    expect(adapter.manifest.adapter_id).toBe(adapterManifest.adapter_id);
    expect(typeof adapter.execute).toBe("function");
  });
});
