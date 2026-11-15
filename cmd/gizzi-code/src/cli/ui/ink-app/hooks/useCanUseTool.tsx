import { feature } from 'bun:bundle';
import { APIUserAbortError } from '@allternit/gizzi-sdk/providers/allternit';
import * as React from 'react';
import { useCallback } from 'react';
import { type AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS, logEvent } from './../services/analytics/index.ts';
import { sanitizeToolNameForAnalytics } from './../services/analytics/metadata.ts';
import type { ToolUseConfirm } from '../components/permissions/PermissionRequest';
import { Text } from '../ink';
import type { ToolPermissionContext, Tool as ToolType, ToolUseContext } from '../Tool';
import { consumeSpeculativeClassifierCheck, peekSpeculativeClassifierCheck } from '../tools/BashTool/bashPermissions';
import { BASH_TOOL_NAME } from '../tools/BashTool/toolName';
import type { ClassifierResult } from '../utils/permissions/bashClassifier';
import type { AssistantMessage } from '../types/message';
import { recordAutoModeDenial } from '../utils/autoModeDenials';
import { clearClassifierChecking, setClassifierApproval, setYoloClassifierApproval } from '../utils/classifierApprovals';
import { logForDebugging } from '../utils/debug';
import { AbortError } from '../utils/errors';
import { logError } from '../utils/log';
import type { PermissionDecision } from '../utils/permissions/PermissionResult';
import { hasPermissionsToUseTool } from '../utils/permissions/permissions';
import { jsonStringify } from '../utils/slowOperations';
import { handleCoordinatorPermission } from './toolPermission/handlers/coordinatorHandler';
import { handleInteractivePermission } from './toolPermission/handlers/interactiveHandler';
import { handleSwarmWorkerPermission } from './toolPermission/handlers/swarmWorkerHandler';
import { createPermissionContext, createPermissionQueueOps } from './toolPermission/PermissionContext';
import { logPermissionDecision } from './toolPermission/permissionLogging';
export type CanUseToolFn<Input extends Record<string, unknown> = Record<string, unknown>> = (tool: ToolType, input: Input, toolUseContext: ToolUseContext, assistantMessage: AssistantMessage, toolUseID: string, forceDecision?: PermissionDecision<Input>) => Promise<PermissionDecision<Input>>;
function useCanUseTool(setToolUseConfirmQueue, setToolPermissionContext) {
  const t0 = async (tool, input, toolUseContext, assistantMessage, toolUseID, forceDecision) => new Promise(resolve => {
      const ctx = createPermissionContext(tool, input, toolUseContext, assistantMessage, toolUseID, setToolPermissionContext, createPermissionQueueOps(setToolUseConfirmQueue));
      if (ctx.resolveIfAborted(resolve)) {
        return;
      }
      const decisionPromise = forceDecision !== undefined ? Promise.resolve(forceDecision) : hasPermissionsToUseTool(tool, input, toolUseContext, assistantMessage, toolUseID);
      return decisionPromise.then(async result => {
        if (result.behavior === "allow") {
          if (ctx.resolveIfAborted(resolve)) {
            return;
          }
          if (feature("TRANSCRIPT_CLASSIFIER") && result.decisionReason?.type === "classifier" && result.decisionReason.classifier === "auto-mode") {
            setYoloClassifierApproval(toolUseID, result.decisionReason.reason);
          }
          ctx.logDecision({
            decision: "accept",
            source: "config"
          });
          resolve(ctx.buildAllow(result.updatedInput ?? input, {
            decisionReason: result.decisionReason
          }));
          return;
        }
        const appState = toolUseContext.getAppState();
        const description = await tool.description(input as never, {
          isNonInteractiveSession: toolUseContext.options.isNonInteractiveSession,
          toolPermissionContext: appState.toolPermissionContext,
          tools: toolUseContext.options.tools
        });
        if (ctx.resolveIfAborted(resolve)) {
          return;
        }
        switch (result.behavior) {
          case "deny":
            {
              logPermissionDecision({
                tool,
                input,
                toolUseContext,
                messageId: ctx.messageId,
                toolUseID
              }, {
                decision: "reject",
                source: "config"
              });
              if (feature("TRANSCRIPT_CLASSIFIER") && result.decisionReason?.type === "classifier" && result.decisionReason.classifier === "auto-mode") {
                recordAutoModeDenial({
                  toolName: tool.name,
                  display: description,
                  reason: result.decisionReason.reason ?? "",
                  timestamp: Date.now()
                });
                toolUseContext.addNotification?.({
                  key: "auto-mode-denied",
                  priority: "immediate",
                  jsx: <><Text color="error">{tool.userFacingName(input).toLowerCase()} denied by auto mode</Text><Text dimColor={true}> · /permissions</Text></>
                });
              }
              resolve(result);
              return;
            }
          case "ask":
            {
              if (appState.toolPermissionContext.awaitAutomatedChecksBeforeDialog) {
                const coordinatorDecision = await handleCoordinatorPermission({
                  ctx,
                  ...(feature("BASH_CLASSIFIER") ? {
                    pendingClassifierCheck: result.pendingClassifierCheck
                  } : {}),
                  updatedInput: result.updatedInput,
                  suggestions: result.suggestions,
                  permissionMode: appState.toolPermissionContext.mode
                });
                if (coordinatorDecision) {
                  resolve(coordinatorDecision);
                  return;
                }
              }
              if (ctx.resolveIfAborted(resolve)) {
                return;
              }
              const swarmDecision = await handleSwarmWorkerPermission({
                ctx,
                description,
                ...(feature("BASH_CLASSIFIER") ? {
                  pendingClassifierCheck: result.pendingClassifierCheck
                } : {}),
                updatedInput: result.updatedInput,
                suggestions: result.suggestions
              });
              if (swarmDecision) {
                resolve(swarmDecision);
                return;
              }
              if (feature("BASH_CLASSIFIER") && result.pendingClassifierCheck && tool.name === BASH_TOOL_NAME && !appState.toolPermissionContext.awaitAutomatedChecksBeforeDialog) {
                const speculativePromise = peekSpeculativeClassifierCheck((input as {
                  command: string;
                }).command);
                if (speculativePromise) {
                  const raceResult = await Promise.race([speculativePromise.then(_temp), new Promise<{
                    type: "timeout";
                  }>(_temp2)]);
                  if (ctx.resolveIfAborted(resolve)) {
                    return;
                  }
                  if (raceResult.type === "result" && raceResult.result.matches && raceResult.result.confidence === "high" && feature("BASH_CLASSIFIER")) {
                    consumeSpeculativeClassifierCheck((input as {
                      command: string;
                    }).command);
                    const matchedRule = raceResult.result.matchedDescription ?? undefined;
                    if (matchedRule) {
                      setClassifierApproval(toolUseID, matchedRule);
                    }
                    ctx.logDecision({
                      decision: "accept",
                      source: {
                        type: "classifier"
                      }
                    });
                    resolve(ctx.buildAllow(result.updatedInput ?? input as Record<string, unknown>, {
                      decisionReason: {
                        type: "classifier" as const,
                        classifier: "bash_allow" as const,
                        reason: `Allowed by prompt rule: "${raceResult.result.matchedDescription}"`
                      }
                    }));
                    return;
                  }
                }
              }
              handleInteractivePermission({
                ctx,
                description,
                result,
                awaitAutomatedChecksBeforeDialog: appState.toolPermissionContext.awaitAutomatedChecksBeforeDialog,
                // Set by the Claude bridge (BRIDGE_MODE builds) or by the live session (useLiveSession).
                bridgeCallbacks: appState.replBridgePermissionCallbacks,
                channelCallbacks: feature("KAIROS") || feature("KAIROS_CHANNELS") ? appState.channelPermissionCallbacks : undefined
              }, resolve);
              return;
            }
        }
      }).catch(error => {
        if (error instanceof AbortError || error instanceof APIUserAbortError) {
          logForDebugging(`Permission check threw ${error.constructor.name} for tool=${tool.name}: ${error.message}`);
          ctx.logCancelled();
          resolve(ctx.cancelAndAbort(undefined, true));
        } else {
          logError(error);
          resolve(ctx.cancelAndAbort(undefined, true));
        }
      }).finally(() => {
        clearClassifierChecking(toolUseID);
      });
    });

  return t0;
}
function _temp2(res: (value: {
  type: "timeout";
}) => void) {
  return setTimeout(res, 2000, {
    type: "timeout" as const
  });
}
function _temp(r: ClassifierResult) {
  return {
    type: "result" as const,
    result: r
  };
}
export default useCanUseTool;
