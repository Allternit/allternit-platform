import type { ToolUseBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/index.mjs';
import React, { useMemo } from 'react';
import { useTerminalSize } from './../../hooks/useTerminalSize.ts';
import type { ThemeName } from './../../utils/theme.ts';
import type { Command } from '../../commands';
import { BLACK_CIRCLE } from '../../constants/figures';
import { stringWidth } from '../../ink/stringWidth';
import { Box, Text, useTheme } from '../../ink';
import { useAppStateMaybeOutsideOfProvider } from '../../state/AppState';
import { findToolByName, type Tool, type ToolProgressData, type Tools } from '../../Tool';
import type { ProgressMessage } from '../../types/message';
import { useIsClassifierChecking } from '../../utils/classifierApprovalsHook';
import { logError } from '../../utils/log';
import type { buildMessageLookups } from '../../utils/messages';
import { MessageResponse } from '../MessageResponse';
import { useSelectedMessageBg } from '../messageActions';
import { SentryErrorBoundary } from '../SentryErrorBoundary';
import { ToolUseLoader } from '../ToolUseLoader';
import { HookProgressMessage } from './HookProgressMessage';
import { ToolUseCard } from './ToolUseCard';
type Props = {
  param: ToolUseBlockParam;
  addMargin: boolean;
  tools: Tools;
  commands: Command[];
  verbose: boolean;
  inProgressToolUseIDs: Set<string>;
  progressMessagesForMessage: ProgressMessage[];
  shouldAnimate: boolean;
  shouldShowDot: boolean;
  inProgressToolCallCount?: number;
  lookups: ReturnType<typeof buildMessageLookups>;
  isTranscriptMode?: boolean;
};
export function AssistantToolUseMessage({
    param,
    addMargin,
    tools,
    commands,
    verbose,
    inProgressToolUseIDs,
    progressMessagesForMessage,
    shouldAnimate,
    shouldShowDot,
    inProgressToolCallCount,
    lookups,
    isTranscriptMode
}: Props) {
  const terminalSize = useTerminalSize();
  const [theme] = useTheme();
  const bg = useSelectedMessageBg();
  const pendingWorkerRequest = useAppStateMaybeOutsideOfProvider(_temp);
  const isClassifierCheckingRaw = useIsClassifierChecking(param.id);
  const permissionMode = useAppStateMaybeOutsideOfProvider(_temp2);
  const hasStrippedRules = useAppStateMaybeOutsideOfProvider(_temp3);
  const isAutoClassifier = permissionMode === "auto" || permissionMode === "plan" && hasStrippedRules;
  const isClassifierChecking = false && isClassifierCheckingRaw && permissionMode !== "auto";
  const parsed = (() => {
    if (!tools) {
      return null;
    }
    const tool = findToolByName(tools, param.name);
    if (!tool) {
      return null;
    }
    const input = tool.inputSchema.safeParse(param.input);
    const data = input.success ? input.data : undefined;
    return {
      tool,
      input,
      userFacingToolName: tool.userFacingName(data),
      userFacingToolNameBackgroundColor: tool.userFacingNameBackgroundColor?.(data),
      isTransparentWrapper: tool.isTransparentWrapper?.() ?? false
    };
  })();
  if (!parsed) {
    logError(new Error(tools ? `Tool ${param.name} not found` : `Tools array is undefined for tool ${param.name}`));
    return null;
  }
  const {
    tool: tool_0,
    input: input_0,
    userFacingToolName,
    userFacingToolNameBackgroundColor,
    isTransparentWrapper
  } = parsed;
  const t2 = lookups.resolvedToolUseIDs.has(param.id);

  const isResolved = t2;
  const t3 = !inProgressToolUseIDs.has(param.id) && !isResolved;

  const isQueued = t3;
  const isWaitingForPermission = pendingWorkerRequest?.toolUseId === param.id;
  if (isTransparentWrapper) {
    if (isQueued || isResolved) {
      return null;
    }
    const t4 = renderToolUseProgressMessage(tool_0, tools, lookups, param.id, progressMessagesForMessage, {
        verbose,
        inProgressToolCallCount,
        isTranscriptMode
      }, terminalSize);

    const t5 = <Box flexDirection="column" width="100%" backgroundColor={bg}>{t4}</Box>;

    return t5;
  }
  if (userFacingToolName === "") {
    return null;
  }
  const t4 = input_0.success ? renderToolUseMessage(tool_0, input_0.data, {
      theme,
      verbose,
      commands
    }) : null;

  const renderedToolUseMessage = t4;
  if (renderedToolUseMessage === null) {
    return null;
  }
  const t5 = addMargin ? 1 : 0;
  const t10 = renderedToolUseMessage !== "" && <Box flexWrap="nowrap"><Text>({renderedToolUseMessage})</Text></Box>;

  const t11 = input_0.success && tool_0.renderToolUseTag && tool_0.renderToolUseTag(input_0.data);

  const t12 = <Box flexDirection="row" flexWrap="nowrap">{t10}{t11}</Box>;

  const t13 = !isResolved && !isQueued && (isClassifierChecking ? <MessageResponse height={1}><Text dimColor={true}>{isAutoClassifier ? "Auto classifier checking\u2026" : "Bash classifier checking\u2026"}</Text></MessageResponse> : isWaitingForPermission ? <MessageResponse height={1}><Text dimColor={true}>Waiting for permission…</Text></MessageResponse> : renderToolUseProgressMessage(tool_0, tools, lookups, param.id, progressMessagesForMessage, {
      verbose,
      inProgressToolCallCount,
      isTranscriptMode
    }, terminalSize));

  const t14 = !isResolved && isQueued && renderToolUseQueuedMessage(tool_0);

  const t15 = <Box flexDirection="column">{t13}{t14}</Box>;

  const isError = lookups.erroredToolUseIDs.has(param.id);
  const t16 = <Box flexDirection="row" justifyContent="space-between" marginTop={t5} width="100%" backgroundColor={bg}><ToolUseCard param={param} toolName={userFacingToolName} isQueued={isQueued} isResolved={isResolved} isError={isError} args={t12}>{t15}</ToolUseCard></Box>;

  return t16;
}
function _temp3(state_1) {
  return !!state_1.toolPermissionContext.strippedDangerousRules;
}
function _temp2(state_0) {
  return state_0.toolPermissionContext.mode;
}
function _temp(state) {
  return state.pendingWorkerRequest;
}
function renderToolUseMessage(tool: Tool, input: unknown, {
  theme,
  verbose,
  commands
}: {
  theme: ThemeName;
  verbose: boolean;
  commands: Command[];
}): React.ReactNode {
  try {
    const parsed = tool.inputSchema.safeParse(input);
    if (!parsed.success) {
      return '';
    }
    return tool.renderToolUseMessage(parsed.data, {
      theme,
      verbose,
      commands
    });
  } catch (error) {
    logError(new Error(`Error rendering tool use message for ${tool.name}: ${error}`));
    return '';
  }
}
function renderToolUseProgressMessage(tool: Tool, tools: Tools, lookups: ReturnType<typeof buildMessageLookups>, toolUseID: string, progressMessagesForMessage: ProgressMessage[], {
  verbose,
  inProgressToolCallCount,
  isTranscriptMode
}: {
  verbose: boolean;
  inProgressToolCallCount?: number;
  isTranscriptMode?: boolean;
}, terminalSize: {
  columns: number;
  rows: number;
}): React.ReactNode {
  const toolProgressMessages = progressMessagesForMessage.filter((msg): msg is ProgressMessage<ToolProgressData> => msg.data.type !== 'hook_progress');
  try {
    const toolMessages = tool.renderToolUseProgressMessage?.(toolProgressMessages, {
      tools,
      verbose,
      terminalSize,
      inProgressToolCallCount: inProgressToolCallCount ?? 1,
      isTranscriptMode
    }) ?? null;
    return <>
        <SentryErrorBoundary>
          <HookProgressMessage hookEvent="PreToolUse" lookups={lookups} toolUseID={toolUseID} verbose={verbose} isTranscriptMode={isTranscriptMode} />
        </SentryErrorBoundary>
        {toolMessages}
      </>;
  } catch (error) {
    logError(new Error(`Error rendering tool use progress message for ${tool.name}: ${error}`));
    return null;
  }
}
function renderToolUseQueuedMessage(tool: Tool): React.ReactNode {
  try {
    return tool.renderToolUseQueuedMessage?.();
  } catch (error) {
    logError(new Error(`Error rendering tool use queued message for ${tool.name}: ${error}`));
    return null;
  }
}
