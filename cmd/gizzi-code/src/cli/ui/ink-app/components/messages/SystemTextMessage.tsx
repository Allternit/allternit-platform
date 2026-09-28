import { Box, Text, type TextProps } from '../../ink';
import { feature } from 'bun:bundle';
import * as React from 'react';
import { useState } from 'react';
import sample from 'lodash-es/sample';
import { BLACK_CIRCLE, REFERENCE_MARK } from '../../constants/figures';
import { OrbMark, ORB_MARK_TEXT } from '../Spinner/OrbMark';
import figures from 'figures';
import { basename } from 'path';
import { MessageResponse } from '../MessageResponse';
import { FilePathLink } from '../FilePathLink';
import { openPath } from '../../utils/browser';
/* eslint-disable @typescript-eslint/no-require-imports */
const teamMemSaved = feature('TEAMMEM') ? require('./teamMemSaved.js') as typeof import('./teamMemSaved.js') : null;
/* eslint-enable @typescript-eslint/no-require-imports */
import { TURN_COMPLETION_VERBS } from '../../constants/turnCompletionVerbs';
import { useTerminalSize } from '../../hooks/useTerminalSize';
import type { SystemMessage, SystemStopHookSummaryMessage, SystemBridgeStatusMessage, SystemTurnDurationMessage, SystemThinkingMessage, SystemMemorySavedMessage, SystemMemoryUpdatedMessage, SystemRunTelemetryMessage, SystemAPIErrorMessage as SystemAPIErrorMessageType } from '../../types/message';
import { MemoryUpdateNotification } from '../memory/MemoryUpdateNotification';
import { SystemAPIErrorMessage } from './SystemAPIErrorMessage';
import { formatDuration, formatNumber, formatSecondsShort } from '../../utils/format';
import { getGlobalConfig } from '../../utils/config';
import { buildRunTelemetryLine } from '../../utils/telemetry/runTelemetryModel';
import Link from '../../ink/components/Link';
import ThemedText from '../design-system/ThemedText';
import { CtrlOToExpand } from '../CtrlOToExpand';
import { useAppState, useAppStateStore } from '../../state/AppState';
import { isBackgroundTask, type TaskState } from '../../tasks/types';
import { getPillLabel } from '../../tasks/pillLabel';
import { useSelectedMessageBg } from '../messageActions';
type Props = {
  message: SystemMessage;
  addMargin: boolean;
  verbose: boolean;
  isTranscriptMode?: boolean;
};
// Referenced only inside a `true ||` dead branch (never evaluated); the
// committed artifact never imported it, so declare rather than import.
declare const HOOK_TIMING_DISPLAY_THRESHOLD_MS: number;

export function SystemTextMessage({
    message,
    addMargin,
    verbose,
    isTranscriptMode
}: Props) {
  const bg = useSelectedMessageBg();
  if (message.subtype === "turn_duration") {
    const t1 = <TurnDurationMessage message={message as SystemTurnDurationMessage} addMargin={addMargin} />;

    return t1;
  }
  if (message.subtype === "run_telemetry") {
    const t1 = <RunTelemetryMessage message={message as SystemRunTelemetryMessage} addMargin={addMargin} />;

    return t1;
  }
  if (message.subtype === "memory_saved") {
    const t1 = <MemorySavedMessage message={message as SystemMemorySavedMessage} addMargin={addMargin} />;

    return t1;
  }
  if (message.subtype === "memory_updated") {
    const t1 = addMargin ? 1 : 0;
    const t2 = <Box minWidth={2}><Text dimColor={true}>{BLACK_CIRCLE}</Text></Box>;

    const t3 = <MemoryUpdateNotification memoryPath={(message as SystemMemoryUpdatedMessage).memoryPath} />;

    const t4 = <Box flexDirection="row" marginTop={t1} backgroundColor={bg} width="100%">{t2}{t3}</Box>;

    return t4;
  }
  if (message.subtype === "away_summary") {
    const t1 = addMargin ? 1 : 0;
    const t2 = <Box minWidth={2}><Text dimColor={true}>{REFERENCE_MARK}</Text></Box>;

    const t3 = <Text dimColor={true}>{message.content}</Text>;

    const t4 = <Box flexDirection="row" marginTop={t1} backgroundColor={bg} width="100%">{t2}{t3}</Box>;

    return t4;
  }
  if (message.subtype === "agents_killed") {
    const t1 = addMargin ? 1 : 0;
    const t2 = <Box minWidth={2}><Text color="error">{BLACK_CIRCLE}</Text></Box>;
    const t3 = <Text dimColor={true}>All background agents stopped</Text>;

    const t4 = <Box flexDirection="row" marginTop={t1} backgroundColor={bg} width="100%">{t2}{t3}</Box>;

    return t4;
  }
  if (message.subtype === "thinking") {
    return null;
  }
  if (message.subtype === "bridge_status") {
    const t1 = <BridgeStatusMessage message={message as SystemBridgeStatusMessage} addMargin={addMargin} />;

    return t1;
  }
  if (message.subtype === "scheduled_task_fire") {
    const t1 = addMargin ? 1 : 0;
    const t2 = <Text dimColor={true}>{ORB_MARK_TEXT} {message.content}</Text>;

    const t3 = <Box marginTop={t1} backgroundColor={bg} width="100%">{t2}</Box>;

    return t3;
  }
  if (message.subtype === "permission_retry") {
    const t1 = addMargin ? 1 : 0;
    const t2 = <Text dimColor={true}>{ORB_MARK_TEXT} </Text>;
    const t3 = <Text>Allowed </Text>;

    const t4 = message.commands.join(", ");

    const t5 = <Text bold={true}>{t4}</Text>;

    const t6 = <Box marginTop={t1} backgroundColor={bg} width="100%">{t2}{t3}{t5}</Box>;

    return t6;
  }
  const isStopHookSummary = message.subtype === "stop_hook_summary";
  if (!isStopHookSummary && !verbose && message.level === "info") {
    return null;
  }
  if (message.subtype === "api_error") {
    const t1 = <SystemAPIErrorMessage message={message as SystemAPIErrorMessageType} verbose={verbose} />;

    return t1;
  }
  if (message.subtype === "stop_hook_summary") {
    const t1 = <StopHookSummaryMessage message={message as SystemStopHookSummaryMessage} addMargin={addMargin} verbose={verbose} isTranscriptMode={isTranscriptMode} />;

    return t1;
  }
  const content = message.content;
  if (typeof content !== "string") {
    return null;
  }
  const t1 = message.level !== "info";
  const t2 = message.level === "warning" ? "warning" : undefined;
  const t3 = message.level === "info";
  const t4 = <Box flexDirection="row" width="100%"><SystemTextMessageInner content={content} addMargin={addMargin} dot={t1} color={t2} dimColor={t3} /></Box>;

  return t4;
}
function StopHookSummaryMessage({
    message,
    addMargin,
    verbose,
    isTranscriptMode
}: {
  message: SystemStopHookSummaryMessage;
  addMargin: boolean;
  verbose: boolean;
  isTranscriptMode?: boolean;
}) {
  const bg = useSelectedMessageBg();
  const {
    hookCount,
    hookInfos,
    hookErrors,
    preventedContinuation,
    stopReason
  } = message;
  const {
    columns
  } = useTerminalSize();
const t1 = message.totalDurationMs ?? hookInfos.reduce(_temp, 0);

  const totalDurationMs = t1;
  if (hookErrors.length === 0 && !preventedContinuation && !message.hookLabel) {
    if (true || totalDurationMs < HOOK_TIMING_DISPLAY_THRESHOLD_MS) {
      return null;
    }
  }
  const t2 = false && totalDurationMs > 0 ? ` (${formatSecondsShort(totalDurationMs)})` : "";

  const totalStr = t2;
  if (message.hookLabel) {
    const t3 = hookCount === 1 ? "hook" : "hooks";
    const t4 = <Text dimColor={true}>{"  \u23BF  "}Ran {hookCount} {message.hookLabel}{" "}{t3}{totalStr}</Text>;

    const t5 = isTranscriptMode && hookInfos.map(_temp2);

    const t6 = <Box flexDirection="column" width="100%">{t4}{t5}</Box>;

    return t6;
  }
  const t3 = addMargin ? 1 : 0;
  const t4 = <Box minWidth={2}><Text>{BLACK_CIRCLE}</Text></Box>;

  const t5 = columns - 10;
  const t6 = <Text bold={true}>{hookCount}</Text>;

  const t7 = message.hookLabel ?? "stop";
  const t8 = hookCount === 1 ? "hook" : "hooks";
  const t9 = !verbose && hookInfos.length > 0 && <>{" "}<CtrlOToExpand /></>;

  const t10 = <Text>Ran {t6} {t7}{" "}{t8}{totalStr}{t9}</Text>;

  const t11 = verbose && hookInfos.length > 0 && hookInfos.map(_temp3);

  const t12 = preventedContinuation && stopReason && <Text><Text dimColor={true}>⎿  </Text>{stopReason}</Text>;

  const t13 = hookErrors.length > 0 && hookErrors.map((err, idx_1) => <Text key={idx_1}><Text dimColor={true}>⎿  </Text>{message.hookLabel ?? "Stop"} hook error: {err}</Text>);

  const t14 = <Box flexDirection="column" width={t5}>{t10}{t11}{t12}{t13}</Box>;

  const t15 = <Box flexDirection="row" marginTop={t3} backgroundColor={bg} width="100%">{t4}{t14}</Box>;

  return t15;
}
function _temp3(info_0, idx_0) {
  const durationStr_0 = false && info_0.durationMs !== undefined ? ` (${formatSecondsShort(info_0.durationMs)})` : "";
  return <Text key={`cmd-${idx_0}`} dimColor={true}>⎿  {info_0.command === "prompt" ? `prompt: ${info_0.promptText || ""}` : info_0.command}{durationStr_0}</Text>;
}
function _temp2(info, idx) {
  const durationStr = false && info.durationMs !== undefined ? ` (${formatSecondsShort(info.durationMs)})` : "";
  return <Text key={`cmd-${idx}`} dimColor={true}>{"     \u23BF "}{info.command === "prompt" ? `prompt: ${info.promptText || ""}` : info.command}{durationStr}</Text>;
}
function _temp(sum, h) {
  return sum + (h.durationMs ?? 0);
}
function SystemTextMessageInner(t0) {
  const {
    content,
    addMargin,
    dot,
    color,
    dimColor
  } = t0;
  const {
    columns
  } = useTerminalSize();
  const bg = useSelectedMessageBg();
  const t1 = addMargin ? 1 : 0;
  const t2 = dot && <Box minWidth={2}><Text color={color} dimColor={dimColor}>{BLACK_CIRCLE}</Text></Box>;

  const t3 = columns - 10;
  const t4 = content.trim();

  const t5 = <Text color={color} dimColor={dimColor} wrap="wrap">{t4}</Text>;

  const t6 = <Box flexDirection="column" width={t3}>{t5}</Box>;

  const t7 = <Box flexDirection="row" marginTop={t1} backgroundColor={bg} width="100%">{t2}{t6}</Box>;

  return t7;
}
function TurnDurationMessage({
    message,
    addMargin
}: {
  message: SystemTurnDurationMessage;
  addMargin: boolean;
}) {
  const bg = useSelectedMessageBg();
  const [verb] = useState(_temp4);
  const store = useAppStateStore();
  const t1 = () => {
      const tasks = store.getState().tasks;
      const running = (Object.values(tasks ?? {}) as TaskState[]).filter(isBackgroundTask);
      return running.length > 0 ? getPillLabel(running) : null;
    };

  const [backgroundTaskSummary] = useState(t1);
  const t2 = getGlobalConfig().showTurnDuration ?? true;

  const showTurnDuration = t2;
  const t3 = formatDuration(message.durationMs);

  const duration = t3;
  const hasBudget = message.budgetLimit !== undefined;
  let t4;
  bb0: {
    if (!hasBudget) {
      t4 = "";
      break bb0;
    }
    const tokens = message.budgetTokens;
    const limit = message.budgetLimit;
    const t5 = tokens >= limit ? `${formatNumber(tokens)} used (${formatNumber(limit)} min ${figures.tick})` : `${formatNumber(tokens)} / ${formatNumber(limit)} (${Math.round(tokens / limit * 100)}%)`;

    const usage = t5;
    const nudges = message.budgetNudges > 0 ? ` \u00B7 ${message.budgetNudges} ${message.budgetNudges === 1 ? "nudge" : "nudges"}` : "";
    t4 = `${showTurnDuration ? " \xB7 " : ""}${usage}${nudges}`;
  }
  const budgetSuffix = t4;
  if (!showTurnDuration && !hasBudget) {
    return null;
  }
  const t5 = addMargin ? 1 : 0;
  const t6 = <OrbMark />;

  const t7 = showTurnDuration && `Gizzi ${verb.toLowerCase()} for ${duration}`;
  const t8 = backgroundTaskSummary && ` \u00B7 ${backgroundTaskSummary} still running`;
  const t9 = <Text dimColor={true}>{t7}{budgetSuffix}{t8}</Text>;

  const t10 = <Box flexDirection="row" marginTop={t5} backgroundColor={bg} width="100%">{t6}{t9}</Box>;

  return t10;
}
function _temp4() {
  return sample(TURN_COMPLETION_VERBS) ?? "Forged";
}
function RunTelemetryMessage({
    message,
    addMargin
}: {
  message: SystemRunTelemetryMessage;
  addMargin: boolean;
}) {
  const bg = useSelectedMessageBg();
  const isBriefOnly = useAppState(s => s.isBriefOnly);
  if (isBriefOnly) {
    return null;
  }
  // One closing line per turn: the settled orb, "Gizzi forged for 3.5s",
  // then the run stats. The duration lives here, not on a second line.
  const [verb] = useState(_temp4);
  const showTurnDuration = getGlobalConfig().showTurnDuration ?? true;
  const forged = showTurnDuration && message.durationMs !== undefined && message.durationMs > 0
    ? `Gizzi ${verb.toLowerCase()} for ${formatDuration(message.durationMs)}`
    : null;
  const line = buildRunTelemetryLine({
    model: message.modelDisplay,
    durationMs: forged ? undefined : message.durationMs,
    inputTokens: message.inputTokens,
    outputTokens: message.outputTokens,
    usageEstimated: message.usageEstimated,
    toolCount: message.toolCount,
    costUSD: message.costUSD,
    contextRatio: message.contextRatio,
    contextEstimated: message.contextEstimated,
    quotaChip: message.quotaChip
  });
  if (!line && !forged) {
    return null;
  }
  const t1 = addMargin ? 1 : 0;
  const t2 = forged ? <OrbMark /> : <Box minWidth={2} />;

  const t3 = <Text dimColor={true}>{[forged, line].filter(Boolean).join(' · ')}</Text>;

  const t4 = <Box flexDirection="row" marginTop={t1} backgroundColor={bg} width="100%">{t2}{t3}</Box>;

  return t4;
}
function MemorySavedMessage({
    message,
    addMargin
}: {
  message: SystemMemorySavedMessage;
  addMargin: boolean;
}) {
  const bg = useSelectedMessageBg();
  const {
    writtenPaths
  } = message;
  const t1 = feature("TEAMMEM") ? teamMemSaved.teamMemSavedPart(message) : null;

  const team = t1;
  const privateCount = writtenPaths.length - (team?.count ?? 0);
  const t2 = privateCount > 0 ? `${privateCount} ${privateCount === 1 ? "memory" : "memories"}` : null;
  const t3 = team?.segment;
  const t4 = [t2, t3].filter(Boolean);

  const parts = t4;
  const t5 = addMargin ? 1 : 0;
  const t6 = <Box minWidth={2}><Text dimColor={true}>{BLACK_CIRCLE}</Text></Box>;

  const t7 = message.verb ?? "Saved";
  const t8 = parts.join(" \xB7 ");
  const t9 = <Box flexDirection="row">{t6}<Text>{t7} {t8}</Text></Box>;

  const t10 = writtenPaths.map(_temp5);

  const t11 = <Box flexDirection="column" marginTop={t5} backgroundColor={bg}>{t9}{t10}</Box>;

  return t11;
}
function _temp5(p) {
  return <MemoryFileRow key={p} path={p} />;
}
function MemoryFileRow(t0) {
  const {
    path
  } = t0;
  const [hover, setHover] = useState(false);
  const t1 = () => void openPath(path);

  const t2 = () => setHover(true);
  const t3 = () => setHover(false);

  const t4 = !hover;
  const t5 = basename(path);

  const t6 = <FilePathLink filePath={path}>{t5}</FilePathLink>;

  const t7 = <Text dimColor={t4} underline={hover}>{t6}</Text>;

  const t8 = <MessageResponse><Box onClick={t1} onMouseEnter={t2} onMouseLeave={t3}>{t7}</Box></MessageResponse>;

  return t8;
}
function ThinkingMessage({
    message,
    addMargin
}: {
  message: SystemThinkingMessage;
  addMargin: boolean;
}) {
  const bg = useSelectedMessageBg();
  const t1 = addMargin ? 1 : 0;
  const t2 = <OrbMark />;

  const t3 = <Text dimColor={true}>{message.content}</Text>;

  const t4 = <Box flexDirection="row" marginTop={t1} backgroundColor={bg} width="100%">{t2}{t3}</Box>;

  return t4;
}
function BridgeStatusMessage({
    message,
    addMargin
}: {
  message: SystemBridgeStatusMessage;
  addMargin: boolean;
}) {
  const bg = useSelectedMessageBg();
  const t1 = addMargin ? 1 : 0;
  const t2 = <Box minWidth={2} />;

  const t3 = <Text><ThemedText color="suggestion">/remote-control</ThemedText> is active. Code in CLI or at</Text>;

  const t4 = <Link url={message.url}>{message.url}</Link>;

  const t5 = message.upgradeNudge && <Text dimColor={true}>⎿ {message.upgradeNudge}</Text>;

  const t6 = <Box flexDirection="column">{t3}{t4}{t5}</Box>;

  const t7 = <Box flexDirection="row" marginTop={t1} backgroundColor={bg} width={999}>{t2}{t6}</Box>;

  return t7;
}
