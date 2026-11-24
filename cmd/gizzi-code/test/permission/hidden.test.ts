// @ts-nocheck
import { test, expect, afterEach } from "bun:test"
import { PermissionNext } from "../../src/runtime/tools/guard/permission/next"
import { Flag } from "../../src/runtime/context/flag/flag"

// Real MCP key form: mcp__<server>__<tool> (buildMcpToolName).
const tools = [
  "bash", "read", "edit", "write", "webfetch", "invalid",
  "mcp__allternit-connectors__phone_text",
  "mcp__allternit-connectors__phone_call",
  "mcp__other__lookup",
]
const sessionRules = [
  { permission: "*", pattern: "*", action: "deny" },
  { permission: "*phone_text*", pattern: "*", action: "allow" },
  { permission: "webfetch", pattern: "*", action: "allow" },
]
const keep = ["webfetch", "mcp__allternit-connectors__phone_text"]
const visible = (rules) => tools.filter((t) => !PermissionNext.hidden(tools, rules).has(t))

const saved = { skip: Flag.GIZZI_SKIP_PERMISSIONS, mode: Flag.GIZZI_PERMISSION_MODE }
afterEach(() => {
  Flag.GIZZI_SKIP_PERMISSIONS = saved.skip
  Flag.GIZZI_PERMISSION_MODE = saved.mode
})

test("deny-all plus allows keeps only the allowed tools", () => {
  const merged = PermissionNext.merge([{ permission: "bash", pattern: "*", action: "allow" }], sessionRules)
  expect(visible(merged).filter((t) => t !== "invalid")).toEqual(keep)
})

test("still hides under GIZZI_SKIP_PERMISSIONS", () => {
  Flag.GIZZI_SKIP_PERMISSIONS = true
  expect(PermissionNext.disabled(tools, sessionRules).size).toBe(0)
  expect(visible(sessionRules).filter((t) => t !== "invalid")).toEqual(keep)
})

test("still hides under dontAsk", () => {
  Flag.GIZZI_PERMISSION_MODE = "dontAsk"
  expect(PermissionNext.disabled(tools, sessionRules).size).toBe(0)
  expect(visible(sessionRules).filter((t) => t !== "invalid")).toEqual(keep)
})

test("edit tools follow the edit permission", () => {
  const h = PermissionNext.hidden(["edit", "write", "read"], [{ permission: "edit", pattern: "*", action: "deny" }])
  expect([...h].sort()).toEqual(["edit", "write"])
})

test("no rules keeps everything; pattern-scoped denies do not hide", () => {
  expect(PermissionNext.hidden(tools, []).size).toBe(0)
  expect(PermissionNext.hidden(tools, [{ permission: "bash", pattern: "rm *", action: "deny" }]).size).toBe(0)
})
