import * as React from 'react';
import { useTerminalSize } from '../../hooks/useTerminalSize';
import { Box, Text } from '../../ink';

/**
 * The rip (spec P3.16): where a conversation moved to a fresh context
 * window. Terminal twin of allternit-ai ThreadRip / SessionRip — same copy
 * and reasons. A torn rule carrying "Fresh context · <time>", then one dim
 * detail line (generation, how full the last window was, why, and where
 * to read the earlier window).
 */
export const HANDOFF_REASONS: Record<string, string> = {
  threshold: 'context was getting full',
  model_switch: 'switched to a model with a smaller window',
  quota: 'paused before a usage limit',
  manual: 'started fresh',
};

export function handoffReasonLabel(reason: string | undefined): string | undefined {
  if (!reason) return undefined;
  return HANDOFF_REASONS[reason] ?? reason.replace(/_/g, ' ');
}

function timeLabel(iso: string | undefined): string {
  const d = iso ? new Date(iso) : new Date();
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
}

const TEAR = '╱╲';

export type SessionRipProps = {
  /** When the new window started (ISO). */
  startedAt?: string;
  /** Generation that starts here (2 = first fresh window). */
  generation?: number;
  /** Why it happened: a HANDOFF_REASONS key or free text. */
  reason?: string;
  /** How full the previous window was (0–1). */
  previousUse?: number | null;
  /** Where the checkpoint and earlier window can be read. */
  hint?: string;
  /** Cap the rule width (e.g. inside a bordered panel). */
  width?: number;
};

export function SessionRip({ startedAt, generation, reason, previousUse, hint, width }: SessionRipProps): React.ReactNode {
  const { columns } = useTerminalSize();
  const label = ` Fresh context · ${timeLabel(startedAt)} `;
  const total = Math.max(label.length + 4, Math.min(width ?? columns - 2, 100));
  const lead = TEAR.repeat(2);
  const tailLen = Math.max(0, total - lead.length - label.length);
  const tail = TEAR.repeat(Math.ceil(tailLen / TEAR.length)).slice(0, tailLen);
  const details = [
    generation ? `gen ${generation}` : undefined,
    previousUse != null && previousUse > 0 ? `${Math.min(100, Math.round(previousUse * 100))}% of the last window used` : undefined,
    handoffReasonLabel(reason),
    hint,
  ].filter(Boolean);
  return <Box flexDirection="column" marginY={1}>
      <Text>
        <Text dimColor>{lead}</Text>
        <Text bold>{label}</Text>
        <Text dimColor>{tail}</Text>
      </Text>
      {details.length > 0 && <Text dimColor wrap="wrap">{'  '}{details.join(' · ')}</Text>}
    </Box>;
}
