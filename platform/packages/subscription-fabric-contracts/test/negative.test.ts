import { describe, expect, it } from "vitest";
import {
  adapterEventSchema,
  artifactTypeSchema,
  capabilityIdSchema,
  failureClassSchema,
  rejectReasonSchema,
  sessionHealthSchema,
  taskInputSchema,
  taskSchema,
  taskStatusSchema,
} from "../src/index";
import { task } from "./fixtures";

describe("negative / rejection tests", () => {
  it("rejects unknown TaskStatus members", () => {
    // "waiting_provider" was deliberately replaced by "waiting_worker" (§S4).
    expect(() => taskStatusSchema.parse("waiting_provider")).toThrow();
    // "artifact_ready" is an event, not a status (§S4).
    expect(() => taskStatusSchema.parse("artifact_ready")).toThrow();
    expect(() => taskStatusSchema.parse("not_a_status")).toThrow();
  });

  it("rejects `selector_not_found` as a FailureClass (folded into provider_ui_changed, §A9)", () => {
    expect(() => failureClassSchema.parse("selector_not_found")).toThrow();
    expect(failureClassSchema.parse("provider_ui_changed")).toBe(
      "provider_ui_changed"
    );
  });

  it("rejects a malformed CapabilityId", () => {
    expect(() => capabilityIdSchema.parse("nodot")).toThrow();
    expect(() => capabilityIdSchema.parse("too.many.dots")).toThrow();
    expect(capabilityIdSchema.parse("good.name")).toBe("good.name");
  });

  it("rejects unknown RejectReason members", () => {
    expect(() => rejectReasonSchema.parse("because_i_said_so")).toThrow();
  });

  it("rejects unknown SessionHealth members", () => {
    expect(() => sessionHealthSchema.parse("sleepy")).toThrow();
  });

  it("rejects unknown ArtifactType members", () => {
    expect(() => artifactTypeSchema.parse("selector")).toThrow();
  });

  it("rejects an unknown AdapterEvent tag", () => {
    expect(() => adapterEventSchema.parse({ t: "artifact_ready" })).toThrow();
    expect(() => adapterEventSchema.parse({ t: "nope" })).toThrow();
  });

  it("rejects a malformed AdapterEvent payload", () => {
    expect(() =>
      adapterEventSchema.parse({ t: "needs_user", reason: "bored", message: "x" })
    ).toThrow();
    expect(() =>
      adapterEventSchema.parse({ t: "done", outcome: "maybe" })
    ).toThrow();
    expect(() => adapterEventSchema.parse({ t: "progress" })).toThrow();
  });

  it("rejects a Task missing a required field", () => {
    const { task_id: _omitted, ...withoutId } = task;
    expect(() => taskSchema.parse(withoutId)).toThrow();
  });

  it("rejects a TaskInput with an unknown variant or malformed payload", () => {
    expect(() => taskInputSchema.parse({ type: "file", path: "x" })).toThrow();
    expect(() => taskInputSchema.parse({ type: "carrier_pigeon" })).toThrow();
    expect(() =>
      taskInputSchema.parse({ type: "image", mime_type: "image/gif", data_base64: "AAAA" })
    ).toThrow();
    expect(() =>
      taskInputSchema.parse({ type: "image", mime_type: "image/png", data_base64: "data:image/png;base64,AAAA" })
    ).toThrow();
    expect(() =>
      taskSchema.parse({ ...task, inputs: [{ type: "text", name: "n" }] })
    ).toThrow();
  });

  it("rejects a TaskAttempt with an unknown submission_state", () => {
    const [attempt] = task.attempts;
    expect(() =>
      taskSchema.parse({
        ...task,
        attempts: [{ ...attempt, submission_state: "probably_sent" }],
      })
    ).toThrow();
  });
});
