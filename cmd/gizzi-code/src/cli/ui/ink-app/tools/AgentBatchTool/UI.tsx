import React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Box, Text } from '../../ink';
import type { ProgressMessage } from '../../types/message';
import type { AgentBatchProgress, Output } from './AgentBatchTool';

export function renderToolUseMessage(input: { mode?: string; csv_path?: string; subagent_type?: string }): React.ReactNode {
  if (!input.csv_path) return '';
  return [input.mode, input.csv_path, input.subagent_type].filter(Boolean).join(' · ');
}

function line(p: { done: number; failed: number; skipped: number; total: number }, running?: number): string {
  const parts = [`${p.done + p.skipped} done`, `${p.failed} failed`];
  if (running !== undefined) parts.push(`${running} running`);
  return `${parts.join(' · ')} of ${p.total}`;
}

/** While it runs: "12 done · 1 failed · 4 running of 300 → results.csv". */
export function renderToolUseProgressMessage(progressMessages: ProgressMessage<AgentBatchProgress>[]): React.ReactNode {
  const data = progressMessages.at(-1)?.data;
  if (!data) {
    return <MessageResponse height={1}>
        <Text dimColor>Starting workers…</Text>
      </MessageResponse>;
  }
  return <MessageResponse height={1}>
      <Text wrap="truncate-middle">
        {data.mode === 'sample' ? 'Sample: ' : ''}
        {line(data, data.running)}
        {data.mode === 'run' ? <Text dimColor> → {data.output_csv_path}</Text> : null}
      </Text>
    </MessageResponse>;
}

export function renderToolResultMessage(output: Output): React.ReactNode {
  const cancelled = output.status === 'cancelled' ? ' (cancelled)' : '';
  if (output.mode === 'sample') {
    return <MessageResponse>
        <Box flexDirection="column">
          <Text>
            Sample: {output.done} of {output.sample?.length ?? 0} ok{cancelled} <Text dimColor>· nothing written</Text>
          </Text>
          {output.sample?.map(row => <Text key={row.id} dimColor={row.status !== 'ok'} wrap="truncate">
              {row.status === 'ok' ? '✓' : '✗'} {row.id}: {row.status === 'ok' ? JSON.stringify(row.values) : row.error}
            </Text>)}
          {output.estimate ? <Text dimColor>
              Full run: {output.estimate.rows} rows, about {output.estimate.tokens.toLocaleString('en-US')} tokens
            </Text> : null}
        </Box>
      </MessageResponse>;
  }
  return <MessageResponse>
      <Box flexDirection="column">
        <Text>
          {line(output)}
          {cancelled}
        </Text>
        <Text dimColor wrap="truncate-middle">{output.output_csv_path}</Text>
        {output.failures?.map(f => <Text key={f.id} color="warning" wrap="truncate">
            ✗ {f.id}: {f.error}
          </Text>)}
      </Box>
    </MessageResponse>;
}
