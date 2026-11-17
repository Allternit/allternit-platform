import React from 'react';
import { Automation } from '@/runtime/primitives/automation';
import { MessageResponse } from '../../components/MessageResponse';
import { Box, Text } from '../../ink';
import type { Output } from './AutomationTool';
import type { Row } from './local';

export function renderToolUseMessage(input: { action?: string; title?: string; id?: string }): React.ReactNode {
  return [input.action === 'run_now' ? 'run now' : input.action, input.title, input.id].filter(Boolean).join(' · ');
}

const WHERE_LABEL: Record<Row['where'], string> = {
  session: 'this session',
  legacy: 'old file',
  account: 'account',
  computer: 'this computer',
};

/** "Wed Nov 18, 09:00" in the user's zone. */
function shortTime(iso: string | null, zone: string): string {
  if (!iso) return '—';
  try {
    return new Intl.DateTimeFormat('en-US', {
      timeZone: zone,
      weekday: 'short',
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      hourCycle: 'h23',
    }).format(new Date(iso));
  } catch {
    return iso.slice(0, 16).replace('T', ' ');
  }
}

function Cell({ width, children, dim }: { width: number; children: React.ReactNode; dim?: boolean }): React.ReactNode {
  return <Box width={width} flexShrink={0} marginRight={1}>
      <Text dimColor={dim} wrap="truncate">{children}</Text>
    </Box>;
}

function Table({ rows, zone }: { rows: Row[]; zone: string }): React.ReactNode {
  return <Box flexDirection="column">
      <Box flexDirection="row">
        <Cell width={32} dim>Title</Cell>
        <Cell width={9} dim>Kind</Cell>
        <Cell width={20} dim>Next run ({zone})</Cell>
        <Cell width={9} dim>Status</Cell>
        <Cell width={13} dim>Where</Cell>
      </Box>
      {rows.map(row => <Box key={`${row.where}:${row.id}`} flexDirection="row">
          <Cell width={32}>{row.title}</Cell>
          <Cell width={9}>{row.kind}</Cell>
          <Cell width={20}>{shortTime(row.nextRunAt, zone)}</Cell>
          <Cell width={9}>{row.status}</Cell>
          <Cell width={13} dim>{WHERE_LABEL[row.where]}</Cell>
        </Box>)}
    </Box>;
}

export function renderToolResultMessage(output: Output): React.ReactNode {
  if (output.action === 'list') {
    const rows = output.rows ?? [];
    return <MessageResponse>
        <Box flexDirection="column">
          {rows.length ? <Table rows={rows} zone={output.zone} /> : <Text dimColor>No scheduled tasks</Text>}
          {output.note ? <Text dimColor>{output.note}</Text> : null}
        </Box>
      </MessageResponse>;
  }
  if (output.status !== 'ok') {
    // The result's opening sentence says why; the rest is for the model.
    const why = output.text.split(' Tell the user', 1)[0];
    return <MessageResponse>
        <Box flexDirection="column">
          <Text>{output.title}</Text>
          <Text dimColor>
            {why}
            {output.status === 'no_scheduler' ? ` Start it with ${Automation.START_COMMAND}.` : ''}
          </Text>
        </Box>
      </MessageResponse>;
  }
  return <MessageResponse>
      <Text>
        {output.title}
        {output.where === 'session' ? <Text dimColor> · this session only</Text> : null}
        {output.where === 'cron' ? <Text dimColor> · this computer</Text> : null}
      </Text>
    </MessageResponse>;
}
