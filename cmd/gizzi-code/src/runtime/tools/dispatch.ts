import { Tool } from "./builtins/tool";
import { HookDispatcher } from "@/runtime/hooks/dispatcher";
import { SettingsHooksBridge } from "@/runtime/hooks/settings-bridge";
import { Instance } from "@/runtime/context/project/instance";
import { Log } from "@/shared/util/log";
import { checkToolHardBan, formatHardBanDenial } from "@/shared/utils/agentHardBans";

export namespace ToolDispatcher {
  const log = Log.create({ service: "tool.dispatcher" });

  // Instance.directory throws outside an Instance.provide context (bare
  // dispatcher-level tests, some ACP/bot paths) — fall back to process.cwd().
  function hookCwd(): string {
    try {
      return Instance.workdir;
    } catch {
      return process.cwd();
    }
  }

  export async function execute(
    tool: Tool.Info,
    args: any,
    ctx: Tool.Context
  ): Promise<any> {
    const initialized = await tool.init()
    return executeInitialized(tool.id, args, ctx, initialized.execute)
  }

  export async function executeInitialized<T>(
    toolID: string,
    args: any,
    ctx: Tool.Context,
    execute: (args: any, ctx: Tool.Context) => Promise<T>,
  ): Promise<T> {
    const sessionId = ctx.sessionID;

    // 0. Dispatch-time hard-ban guard: the active agent's character-card hard
    // bans (ALLTERNIT_AGENT_HARD_BANS) block banned tool calls (e.g.
    // email_send → send_agent_email, gmail.send_email, allternit_mail.send)
    // before hooks or execution. See src/shared/utils/agentHardBans.ts.
    const banViolation = checkToolHardBan(toolID, args)
    if (banViolation) {
      log.warn("Tool call blocked by agent hard ban", { toolId: toolID, category: banViolation.category, matched: banViolation.matched });
      const denied = {
        title: "Blocked by agent policy",
        output: formatHardBanDenial(banViolation),
        metadata: { denied: true, policyViolation: banViolation }
      } as T;
      await HookDispatcher.emit({
        name: "PostToolUseFailure",
        timestamp: Date.now(),
        sessionId,
        payload: { tool: toolID, args, reason: banViolation.reason, blocked: true },
      })
      await SettingsHooksBridge.runToolEvent("PostToolUseFailure", {
        sessionId,
        cwd: hookCwd(),
        toolName: toolID,
        toolInput: args,
        error: banViolation.reason,
      });
      return denied
    }

    // 1. Emit PreToolUse Hook
    const hookRes = await HookDispatcher.emit({
      name: "PreToolUse",
      timestamp: Date.now(),
      sessionId,
      payload: { tool: toolID, args, context: ctx }
    });

    if (hookRes.decision === "deny") {
      log.warn("Tool usage denied by hook", { toolId: toolID, reason: hookRes.reason });
      const denied = {
        title: "Access Denied",
        output: `Tool usage was denied by a security policy: ${hookRes.reason || "No reason provided."}`,
        metadata: { denied: true }
      } as T;
      await HookDispatcher.emit({
        name: "PostToolUseFailure",
        timestamp: Date.now(),
        sessionId,
        payload: { tool: toolID, args, reason: hookRes.reason, blocked: true },
      })
      return denied
    }

    if (hookRes.modifiedPayload) {
      args = hookRes.modifiedPayload.args ?? hookRes.modifiedPayload;
    }

    // 1b. Settings.json hooks (PreToolUse) — Claude-Code-style command/http
    // hooks from settings files, bridged onto the runtime path. A deny gates
    // the tool call with the same structured denial as the gizzi-config
    // hooks; updatedInput replaces the args (ink-app replace semantics);
    // "ask" routes through the normal PermissionNext prompt via ctx.ask.
    const settingsPre = await SettingsHooksBridge.runToolEvent("PreToolUse", {
      sessionId,
      cwd: hookCwd(),
      toolName: toolID,
      toolInput: args,
    });
    if (settingsPre.decision === "deny") {
      log.warn("Tool usage denied by settings hook", { toolId: toolID, reason: settingsPre.reason });
      const denied = {
        title: "Access Denied",
        output: `Tool usage was denied by a settings hook: ${settingsPre.reason || "No reason provided."}`,
        metadata: { denied: true }
      } as T;
      await HookDispatcher.emit({
        name: "PostToolUseFailure",
        timestamp: Date.now(),
        sessionId,
        payload: { tool: toolID, args, reason: settingsPre.reason, blocked: true },
      })
      await SettingsHooksBridge.runToolEvent("PostToolUseFailure", {
        sessionId,
        cwd: hookCwd(),
        toolName: toolID,
        toolInput: args,
        error: settingsPre.reason ?? "denied by settings hook",
      });
      return denied
    }

    if (settingsPre.updatedInput) {
      log.info("Settings hook rewrote tool input", { toolId: toolID, keys: Object.keys(settingsPre.updatedInput) });
      args = settingsPre.updatedInput;
    }

    if (settingsPre.decision === "ask") {
      // Force the normal downstream permission prompt: same ctx.ask flow the
      // tool itself would trigger. A rejection lands as the same structured
      // denial as a hook deny — never silently allowed.
      try {
        await ctx.ask({
          permission: toolID,
          patterns: SettingsHooksBridge.deriveAskPatterns(args),
          metadata: { settingsHookAsk: true, reason: settingsPre.reason },
        } as any);
      } catch (askError) {
        const reason = askError instanceof Error ? askError.message : String(askError);
        log.warn("Tool usage denied at settings-hook-forced permission prompt", { toolId: toolID, reason });
        const denied = {
          title: "Access Denied",
          output: `Tool usage was denied at the permission prompt: ${reason || "No reason provided."}`,
          metadata: { denied: true }
        } as T;
        await HookDispatcher.emit({
          name: "PostToolUseFailure",
          timestamp: Date.now(),
          sessionId,
          payload: { tool: toolID, args, reason, blocked: true },
        })
        await SettingsHooksBridge.runToolEvent("PostToolUseFailure", {
          sessionId,
          cwd: hookCwd(),
          toolName: toolID,
          toolInput: args,
          error: reason,
        });
        return denied
      }
    }

    try {
      const result = await execute(args, ctx);

      // 3. Emit PostToolUse Hook
      await HookDispatcher.emit({
        name: "PostToolUse",
        timestamp: Date.now(),
        sessionId,
        payload: { tool: toolID, args, result }
      });
      await SettingsHooksBridge.runToolEvent("PostToolUse", {
        sessionId,
        cwd: hookCwd(),
        toolName: toolID,
        toolInput: args,
        toolResponse: result,
      });

      return result;
    } catch (error) {
      log.error("Tool execution failed", { toolId: toolID, error });

      // 4. Emit ToolError Hook
      await HookDispatcher.emit({
        name: "PostToolUseFailure",
        timestamp: Date.now(),
        sessionId,
        payload: { tool: toolID, args, error }
      });
      await SettingsHooksBridge.runToolEvent("PostToolUseFailure", {
        sessionId,
        cwd: hookCwd(),
        toolName: toolID,
        toolInput: args,
        error: error instanceof Error ? error.message : String(error),
      });

      throw error;
    }
  }
}
