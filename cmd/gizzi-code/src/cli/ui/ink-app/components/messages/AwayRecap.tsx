import * as React from 'react';
import { Box, Text } from '../../ink';
import type { SystemAwaySummaryMessage } from '../../types/message';
import { getDisplayPath } from '../../utils/file';
import { formatDuration } from '../../utils/format';

const MAX_FILES = 5;

/**
 * The "while you were away" recap: what the task is, files changed with
 * +/- counts (from the edit tool results), what still fails, and the next
 * step. The model writes "<summary>\nFailing: …\nNext: …"; older recaps are
 * plain sentences and render as one bullet.
 */
export function AwayRecap({
  message,
  addMargin,
  bg,
}: {
  message: SystemAwaySummaryMessage;
  addMargin: boolean;
  bg?: string;
}): React.ReactNode {
  const lines = message.content.split('\n').map(l => l.trim()).filter(Boolean);
  const failing = lines.find(l => /^failing:/i.test(l))?.replace(/^failing:\s*/i, '');
  const next = lines.find(l => /^next:/i.test(l))?.replace(/^next:\s*/i, '');
  const summary = lines.filter(l => !/^(failing|next):/i.test(l));
  const files = message.files ?? [];
  const failingItems = failing && !/^(none|n\/a|-)\.?$/i.test(failing) ? failing.split(/,\s*/).filter(Boolean) : [];
  return (
    <Box flexDirection="column" marginTop={addMargin ? 1 : 0} backgroundColor={bg} width="100%">
      <Text>
        <Text color="gizzi" bold>Recap</Text>
        {message.awayMs !== undefined && message.awayMs >= 60_000 ? (
          <Text dimColor> (you were away {formatDuration(message.awayMs, { mostSignificantOnly: true })})</Text>
        ) : null}
      </Text>
      {summary.map((line, i) => (
        <Text key={`s${i}`}>  • {line}</Text>
      ))}
      {files.slice(0, MAX_FILES).map(f => (
        <Text key={f.path}>
          {'  • '}
          <Text color="suggestion">{getDisplayPath(f.path)}</Text>
          <Text color="success"> +{f.added}</Text>
          <Text color="error"> −{f.removed}</Text>
        </Text>
      ))}
      {files.length > MAX_FILES ? <Text dimColor>    and {files.length - MAX_FILES} more files</Text> : null}
      {failingItems.length > 0 ? (
        <Text>
          {'  • '}
          <Text color="error">
            {failingItems.length === 1 ? '1 still fails:' : `${failingItems.length} still fail:`}
          </Text>{' '}
          {failingItems.join(', ')}
        </Text>
      ) : null}
      {next ? (
        <Text>
          {'  '}
          <Text dimColor>Next:</Text> {next}
        </Text>
      ) : null}
    </Box>
  );
}
