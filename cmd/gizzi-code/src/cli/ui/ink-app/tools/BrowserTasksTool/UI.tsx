import React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Text } from '../../ink';
import type { Output } from './BrowserTasksTool';

export function renderToolUseMessage(input: { action?: string; tasks?: Array<{ id: string }> }): React.ReactNode {
  return `${input.action ?? 'run'} ${(input.tasks ?? []).map(t => t.id).join(', ')}`;
}

const COLOR = { done: 'green', needs_login: 'yellow', needs_user: 'yellow', failed: 'red' } as const;

// One line per task: status, result summary, final URL.
export function renderToolResultMessage(output: Output): React.ReactNode {
  return <MessageResponse>
      {output.results.map(r => <Text key={r.id}>
          <Text color={COLOR[r.status]}>{r.status}</Text> {r.id}: {r.result.replace(/\s+/g, ' ').slice(0, 100)}
          {r.final_url ? <Text dimColor> · {r.final_url}</Text> : null}
        </Text>)}
    </MessageResponse>;
}
