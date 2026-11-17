import React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Text } from '../../ink';
import type { Output } from './FolderAccessTool';

export function renderToolUseMessage(input: { action?: string; paths?: string[] }): React.ReactNode {
  const paths = input.paths?.join(', ') ?? '';
  return input.action === 'request_delete' ? `delete in ${paths}` : paths;
}

export function renderToolResultMessage(output: Output): React.ReactNode {
  const verb = output.action === 'request_delete' ? 'Deletes allowed in' : 'Access granted to';
  return <MessageResponse>
      <Text>
        {verb} <Text bold>{output.granted.join(', ')}</Text>
      </Text>
    </MessageResponse>;
}
