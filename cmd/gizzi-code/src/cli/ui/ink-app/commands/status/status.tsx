import * as React from 'react';
import type { LocalJSXCommandContext } from '../../commands';
import { Settings } from '../../components/Settings/Settings';
import type { LocalJSXCommandOnDone } from '../../types/command';
import type { Message } from '../../types/message.js';
import {
  getCwdState,
  getOriginalCwd,
  getSessionId,
  getTotalCostUSD,
  getTotalDuration,
  getTotalInputTokens,
  getTotalOutputTokens,
  getTotalCacheReadInputTokens,
  getTotalCacheCreationInputTokens,
} from '../../bootstrap/state.js';
import { formatDuration, formatTokens } from '../../utils/format.js';
import {
  calculateContextPercentages,
  getContextWindowForModel,
} from '../../utils/context.js';
import { getCurrentUsage, getTokenUsage } from '../../utils/tokens.js';
import {
  getRuntimeMainLoopModel,
  renderModelName,
} from '../../utils/model/model.js';
import {
  buildSessionStatus,
  countUserTurns,
  formatServicesLine,
  getAuthMethodDescription,
  getServicesLine,
} from '../../utils/statusModel.js';

const PROGRESS_BAR_WIDTH = 24;

function renderProgressBar(ratio: number, width: number): string {
  const clamped = Math.max(0, Math.min(1, ratio));
  const filled = Math.round(clamped * width);
  const empty = width - filled;
  return '█'.repeat(filled) + '░'.repeat(empty);
}

function renderInlineStatus(context: LocalJSXCommandContext, services: string | null = null): string {
  // TODO(types): AppState has no `messages` field (upstream drift shared with
  // dash.ts/session-info.tsx/statusModel.ts); runtime value is always
  // undefined, so the `?? []` fallback was always in effect.
  const messages = (context.getAppState() as { messages?: Message[] }).messages ?? [];
  const model = getRuntimeMainLoopModel({
    permissionMode: context.getAppState().toolPermissionContext.mode,
    mainLoopModel: context.options.mainLoopModel,
    exceeds200kTokens: false,
  });

  const modelDisplay = renderModelName(model);
  const cwd = getCwdState();
  const projectDir = getOriginalCwd();
  const sessionId = getSessionId();
  const version = (typeof MACRO !== 'undefined' ? MACRO.VERSION : undefined) ?? 'unknown';

  const contextWindowSize = getContextWindowForModel(model, undefined);
  const currentUsage = getCurrentUsage(messages);
  const contextPercentages = calculateContextPercentages(
    currentUsage,
    contextWindowSize,
  );

  const totalCost = getTotalCostUSD();
  const totalDuration = getTotalDuration();
  const totalInput = getTotalInputTokens();
  const totalOutput = getTotalOutputTokens();
  const cacheRead = getTotalCacheReadInputTokens();
  const cacheCreation = getTotalCacheCreationInputTokens();

  const mcpClients = context.options.mcpClients ?? [];

  const lines: string[] = [];
  lines.push('');
  lines.push('╭────────────────────────────────────────╮');
  lines.push('│  Status                                │');
  lines.push('╰────────────────────────────────────────╯');
  lines.push('');
  lines.push(`  Model:     ${modelDisplay}`);
  lines.push(`  Directory: ${cwd}`);
  if (projectDir && projectDir !== cwd) {
    lines.push(`  Project:   ${projectDir}`);
  }
  lines.push(`  Session:   ${sessionId}`);
  lines.push(`  Version:   ${version}`);
  lines.push(`  Auth:      ${getAuthMethodDescription()}`);
  lines.push(`  Turns:     ${countUserTurns(messages)}`);
  if (services) {
    lines.push(`  Services:  ${services}`);
  }
  lines.push('');

  lines.push('Context window');
  if (contextPercentages.used !== null && currentUsage) {
    const totalInputNow =
      currentUsage.input_tokens +
      currentUsage.cache_creation_input_tokens +
      currentUsage.cache_read_input_tokens;
    const ratio = contextPercentages.used / 100;
    const bar = renderProgressBar(ratio, PROGRESS_BAR_WIDTH);
    lines.push(
      `  ${bar}  ${contextPercentages.used}% (${formatTokens(totalInputNow)} / ${formatTokens(contextWindowSize)})`,
    );
  } else {
    lines.push('  No context usage data available.');
  }

  lines.push('');
  lines.push('Session usage');
  lines.push(`  Cost:      $${totalCost.toFixed(4)}`);
  lines.push(`  Duration:  ${formatDuration(totalDuration, { mostSignificantOnly: true })}`);
  lines.push(`  Requests:  ${messages.filter(m => getTokenUsage(m) !== undefined).length}`);
  lines.push(`  Tokens:    ${formatTokens(totalInput + totalOutput)}`);
  lines.push(`             ${formatTokens(totalInput)} in / ${formatTokens(totalOutput)} out`);
  if (cacheRead > 0 || cacheCreation > 0) {
    lines.push(`             ${formatTokens(cacheRead)} cache read / ${formatTokens(cacheCreation)} cache write`);
  }

  if (mcpClients.length > 0) {
    lines.push('');
    lines.push(`MCP servers: ${mcpClients.length} connected`);
    for (const client of mcpClients.slice(0, 5)) {
      const name = client.name ?? 'unknown';
      // TODO(types): MCPServerConnection has no `status` field (discriminator
      // is `type`); runtime value is always undefined → 'unknown'.
      const status = (client as { status?: string }).status ?? 'unknown';
      lines.push(`  • ${name} (${status})`);
    }
    if (mcpClients.length > 5) {
      lines.push(`  ... and ${mcpClients.length - 5} more`);
    }
  }

  lines.push('');

  return lines.join('\n');
}

export async function call(
  onDone: LocalJSXCommandOnDone,
  context: LocalJSXCommandContext,
  args: string,
): Promise<React.ReactNode> {
  const trimmed = args.trim();
  if (trimmed === '--json') {
    const status = await buildSessionStatus(context);
    onDone(JSON.stringify(status, null, 2), {
      display: 'system',
    });
    return null;
  }
  if (trimmed === '--inline') {
    onDone(renderInlineStatus(context, formatServicesLine(await getServicesLine())), { display: 'system' });
    return null;
  }
  return <Settings onClose={onDone} context={context} defaultTab="Status" />;
}
