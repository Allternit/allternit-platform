// Pins the wire enums/transition tables to cmd/allternit-api/src/agent_gateway_routes.rs (CONNECTION_STATES ..
// exec_next/remote_next, parity_of). If Rust changes, this fails until the contracts follow.
import { describe, expect, it } from "vitest";
import {
  CONNECTION_TRANSITIONS, EXECUTION_TRANSITIONS, REMOTE_THREAD_TRANSITIONS,
  channelSyncStateSchema, connectionStateSchema, botExecutionStateSchema, remoteThreadStateSchema, packGapSeveritySchema, packParity,
} from "../src";

describe("contracts == allternit-api constants", () => {
  it("state lists", () => {
    expect(connectionStateSchema.options).toEqual(["DISCONNECTED", "CONSENT_REQUIRED", "AUTHENTICATING", "VERIFYING", "CONNECTED", "DEGRADED", "AUTH_FAILED", "EXPIRED", "REVOKED", "BLOCKED"]);
    expect(botExecutionStateSchema.options).toEqual(["UNBOUND", "BOUND", "READY", "DEGRADED", "NEEDS_AUTH", "PAUSED", "DISABLED", "FAILED"]);
    expect(remoteThreadStateSchema.options).toEqual(["UNBOUND", "OPENING", "ACTIVE", "HANDOFF_PENDING", "CLOSED"]);
    expect(channelSyncStateSchema.options).toEqual(["LIVE", "DELAYED", "RECONNECTING", "DEGRADED", "DISCONNECTED"]);
    expect(packGapSeveritySchema.options).toEqual(["visual_parity", "functional", "data_loss"]);
  });
  it("transition tables (Rust match arms)", () => {
    expect(EXECUTION_TRANSITIONS.READY).toEqual(["DEGRADED", "NEEDS_AUTH", "PAUSED", "DISABLED", "FAILED", "BOUND"]);
    expect(EXECUTION_TRANSITIONS.FAILED).toEqual(["BOUND", "UNBOUND"]);
    expect(CONNECTION_TRANSITIONS.REVOKED).toEqual(["CONSENT_REQUIRED", "DISCONNECTED"]);
    expect(CONNECTION_TRANSITIONS.DEGRADED).toEqual(["CONNECTED", "EXPIRED", "REVOKED", "BLOCKED", "DISCONNECTED"]);
    expect(REMOTE_THREAD_TRANSITIONS.HANDOFF_PENDING).toEqual(["ACTIVE", "CLOSED"]);
    expect(REMOTE_THREAD_TRANSITIONS.CLOSED).toEqual([]);
  });
  it("parity_of: blocked on open data_loss/functional, partial on visual only, full when none open", () => {
    expect(packParity([{ severity: "functional", status: "open" }])).toBe("blocked");
    expect(packParity([{ severity: "visual_parity", status: "open" }, { severity: "data_loss", status: "resolved" }])).toBe("partial");
    expect(packParity([{ severity: "data_loss", status: "wontfix" }])).toBe("full");
  });
});
