import React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Box, Text } from '../../ink';

type Input = {
  action?: 'read' | 'write';
  keys?: string[];
  changes?: { key: string; value: unknown }[];
};

export function renderToolUseMessage(input: Input): React.ReactNode {
  if (input.action === 'write') {
    return (input.changes ?? []).map(c => `${c.key} = ${JSON.stringify(c.value)}`).join(', ');
  }
  return input.keys?.length ? input.keys.join(', ') : 'all settings';
}

export function renderToolResultMessage(output: { summary: string; lines?: string[] }): React.ReactNode {
  return <MessageResponse>
      <Box flexDirection="column">
        <Text>{output.summary}</Text>
        {(output.lines ?? []).map((line, i) => <Text key={i} dimColor>{line}</Text>)}
      </Box>
    </MessageResponse>;
}
