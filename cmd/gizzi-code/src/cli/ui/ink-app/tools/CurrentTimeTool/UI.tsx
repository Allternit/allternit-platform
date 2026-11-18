import React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Text } from '../../ink';
import type { Output } from './CurrentTimeTool';

export function renderToolUseMessage(input: { timezone?: string }): React.ReactNode {
  return input.timezone ?? '';
}

export function renderToolResultMessage(output: Output): React.ReactNode {
  const time = output.iso.slice(11, 16);
  return <MessageResponse>
      <Text>
        {output.weekday} {time} {output.time_zone} (UTC{output.utc_offset})
        {output.source === 'process' ? <Text dimColor> · this machine's zone</Text> : null}
      </Text>
    </MessageResponse>;
}
