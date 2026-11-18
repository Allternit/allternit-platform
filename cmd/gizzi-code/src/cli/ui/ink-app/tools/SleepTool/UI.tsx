import React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Text } from '../../ink';
import type { ProgressMessage } from '../../types/message';
import type { Output, SleepProgress } from './SleepTool';

export function renderToolUseMessage(input: { seconds?: number; reason?: string }): React.ReactNode {
  if (input.seconds === undefined) return '';
  return `${Math.round(input.seconds)}s${input.reason ? ` — ${input.reason}` : ''}`;
}

/** While it waits: "Waiting 30s — <reason>". */
export function renderToolUseProgressMessage(progressMessages: ProgressMessage<SleepProgress>[]): React.ReactNode {
  const data = progressMessages.at(-1)?.data;
  if (!data) return null;
  return <MessageResponse height={1}>
      <Text dimColor>
        Waiting {data.seconds}s{data.reason ? ` — ${data.reason}` : ''} · type to wake it
      </Text>
    </MessageResponse>;
}

const WOKE: Record<Output['woke'], string> = {
  timer: 'time up',
  user_input: 'woke on your message',
  cancelled: 'cancelled',
};

export function renderToolResultMessage(output: Output): React.ReactNode {
  return <MessageResponse>
      <Text>
        Waited {output.slept_seconds}s <Text dimColor>· {WOKE[output.woke]}</Text>
      </Text>
    </MessageResponse>;
}
