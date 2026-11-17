import React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Text } from '../../ink';

export function renderToolUseMessage(input: Record<string, unknown>): React.ReactNode {
  const thread = typeof input.thread === 'string' ? input.thread : '';
  return thread;
}

export function renderToolResultMessage(output: { summary: string }): React.ReactNode {
  return <MessageResponse>
      <Text>{output.summary}</Text>
    </MessageResponse>;
}
