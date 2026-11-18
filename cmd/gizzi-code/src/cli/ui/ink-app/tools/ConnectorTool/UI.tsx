import React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Box, Text } from '../../ink';

export function renderSearchUseMessage(input: { query?: string; load?: boolean }): React.ReactNode {
  if (!input.query) return null;
  return input.load ? `${input.query} (load)` : input.query;
}

export function renderSuggestUseMessage(input: { ids?: string[] }): React.ReactNode {
  return input.ids?.join(', ') ?? '';
}

export function renderToolResultMessage(output: { summary: string; card?: string[]; reason?: string }): React.ReactNode {
  if (!output.card?.length) {
    return <MessageResponse>
        <Text>{output.summary}</Text>
      </MessageResponse>;
  }
  // The TUI's suggestion card: plain text, no links or codes. The user connects in the app.
  return <MessageResponse>
      <Box flexDirection="column" borderStyle="round" paddingX={1}>
        {output.reason ? <Text dimColor>{output.reason}</Text> : null}
        {output.card.map(line => <Text key={line}>{line}</Text>)}
      </Box>
    </MessageResponse>;
}
