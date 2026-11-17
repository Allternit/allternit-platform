/**
 * settings (Primitives Wave D, D2): read and change the user's settings.
 * Core in runtime/primitives/settings.ts; keys and risk classes in
 * contracts/settings/definitions.json. Every client. Permission: allow
 * (loop/agent.ts); the action is the pattern, and the plan agent denies
 * `write`, so writes are refused in plan mode. The settings card (Undo,
 * Confirm, Open Settings) rides in the tool_result metadata, as in Wave A.
 *
 * W2 stub: final id and parameters; lane S fills the execute body.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { Settings } from "@/runtime/primitives/settings"
import DESCRIPTION from "@/runtime/tools/builtins/settings.txt"

export const SettingsTool = Tool.define("settings", {
  description: DESCRIPTION,
  parameters: Settings.Input,
  async execute(params, ctx) {
    await ctx.ask({
      permission: "settings",
      patterns: [params.action],
      always: [params.action],
      metadata: { action: params.action, keys: params.keys, changes: params.changes },
    })
    try {
      const r =
        params.action === "write"
          ? await Settings.write(ctx.sessionID, ctx.callID ?? "", params.changes ?? [], ctx.abort)
          : await Settings.read(ctx.sessionID, params.keys, ctx.abort)
      return {
        title: params.action === "write" ? "Settings change" : "Settings",
        output: JSON.stringify(r),
        metadata: { action: params.action as string, changes: r.changes ?? [] },
      }
    } catch (error) {
      return {
        title: "Settings unavailable",
        output: error instanceof Error ? error.message : String(error),
        metadata: { action: params.action as string, changes: [] as Settings.ChangeResult[] },
      }
    }
  },
})
