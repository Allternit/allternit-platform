import React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Text } from '../../ink';
import type { Output } from './StartSessionTool';

export function renderToolUseMessage(input: { device?: string; folder?: string }): React.ReactNode {
  return `${input.device ?? 'this computer'}: ${input.folder ?? ''}`;
}

export function renderToolResultMessage(output: Output): React.ReactNode {
  return <MessageResponse>
      <Text>{output.line}</Text>
    </MessageResponse>;
}
