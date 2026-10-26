// @ts-nocheck
/**
 * A:// store consolidation boundary (P-T1) tests for the gizzi Cowork store.
 *
 * In `canonical` mode (gizzi paired with an Allternit API) every local
 * run/job/event write is gated off with a clear error pointing at the
 * fabric-transport endpoints. In default `legacy` mode (standalone gizzi)
 * writes still work but the boundary documents them as the local projection.
 */
import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import {
  CoworkStoreBoundaryError,
  assertCoworkWriteAllowed,
  isLegacyProjectionMode,
  resetCoworkStoreBoundaryForTests,
  resolveCoworkStoreMode,
} from "../../src/runtime/cowork/store-boundary"
import { RunService, ScheduleService, ApprovalService, CheckpointService } from "../../src/runtime/cowork/cowork.service"

// Other files may have resolved and cached the mode already.
beforeEach(() => {
  resetCoworkStoreBoundaryForTests()
})

afterEach(() => {
  delete process.env.GIZZI_COWORK_STORE
  delete process.env.ALLTERNIT_COWORK_CANONICAL
  resetCoworkStoreBoundaryForTests()
})

describe("mode resolution", () => {
  test("explicit GIZZI_COWORK_STORE wins", () => {
    process.env.GIZZI_COWORK_STORE = "canonical"
    expect(resolveCoworkStoreMode()).toBe("canonical")
  })

  test("ALLTERNIT_COWORK_CANONICAL=1 implies canonical", () => {
    process.env.ALLTERNIT_COWORK_CANONICAL = "1"
    expect(resolveCoworkStoreMode()).toBe("canonical")
    expect(isLegacyProjectionMode()).toBe(false)
  })

  test("default is legacy (standalone = local store is canonical)", () => {
    expect(resolveCoworkStoreMode()).toBe("legacy")
    expect(isLegacyProjectionMode()).toBe(true)
  })
})

describe("write gating", () => {
  test("canonical mode refuses writes with the boundary error", () => {
    process.env.GIZZI_COWORK_STORE = "canonical"
    expect(() => assertCoworkWriteAllowed("run")).toThrow(CoworkStoreBoundaryError)
    try {
      assertCoworkWriteAllowed("run")
    } catch (e) {
      expect((e as Error).message).toContain("/api/v1/fabric/transport/intents")
      expect((e as Error).message).toContain("GIZZI_COWORK_STORE=legacy")
    }
  })

  test("legacy mode allows writes", () => {
    expect(() => assertCoworkWriteAllowed("run")).not.toThrow()
  })

  test("RunService.create is gated in canonical mode", () => {
    process.env.GIZZI_COWORK_STORE = "canonical"
    expect(() => RunService.create({ name: "boundary-proof", mode: "local" })).toThrow(
      CoworkStoreBoundaryError,
    )
  })

  test("RunService.updateStatus is gated in canonical mode", () => {
    process.env.GIZZI_COWORK_STORE = "canonical"
    expect(() => RunService.updateStatus("run-nope", "running")).toThrow(CoworkStoreBoundaryError)
  })

  test("RunService.appendEvent is gated in canonical mode", () => {
    process.env.GIZZI_COWORK_STORE = "canonical"
    expect(() => RunService.appendEvent("run-nope", "evt", {})).toThrow(CoworkStoreBoundaryError)
  })

  test("ScheduleService.create is gated in canonical mode", () => {
    process.env.GIZZI_COWORK_STORE = "canonical"
    expect(() =>
      ScheduleService.create({ name: "s", cron_expr: "0 * * * *", job_template: { command: "echo hi" } }),
    ).toThrow(CoworkStoreBoundaryError)
  })

  test("ApprovalService.create is gated in canonical mode", () => {
    process.env.GIZZI_COWORK_STORE = "canonical"
    expect(() => ApprovalService.create({ run_id: "run-x", title: "approve?" })).toThrow(
      CoworkStoreBoundaryError,
    )
  })

  test("CheckpointService.create is gated in canonical mode", () => {
    process.env.GIZZI_COWORK_STORE = "canonical"
    expect(() => CheckpointService.create({ run_id: "run-x" })).toThrow(CoworkStoreBoundaryError)
  })
})
