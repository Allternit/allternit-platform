import React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Text } from '../../ink';

export function renderToolUseMessage(input: { action?: string; ref?: string }): React.ReactNode {
  return input.ref ? `${input.action} ${input.ref}` : 'list refs';
}

export function renderToolResultMessage(output: { summary: string }): React.ReactNode {
  return <MessageResponse>
      <Text>{output.summary}</Text>
    </MessageResponse>;
}
