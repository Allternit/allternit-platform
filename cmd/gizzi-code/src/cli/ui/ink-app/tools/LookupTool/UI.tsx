import React from 'react';
import type { LookupCard } from '@/runtime/primitives/lookup';
import { MessageResponse } from '../../components/MessageResponse';
import { Box, Text } from '../../ink';
import { cardText } from './cardText';
import type { Output } from './LookupTool';

export function renderToolUseMessage(input: { kind?: string; query?: string; place?: string }): React.ReactNode {
  if (!input.kind) return '';
  const where = input.place ? ` · ${input.place}` : '';
  return `${input.kind}${input.query ? `: ${input.query}` : ''}${where}`;
}

function CardView({ card }: { card: LookupCard }): React.ReactNode {
  const { lines, credit } = cardText(card);
  return <Box flexDirection="column">
      <Text bold>{card.title}</Text>
      {lines.map((line, i) => <Text key={i}>{line || ' '}</Text>)}
      {credit ? <Text dimColor>{credit}</Text> : null}
    </Box>;
}

export function renderToolResultMessage(output: Output): React.ReactNode {
  if (output.error) {
    return <MessageResponse>
        <Text color="warning">{output.text}</Text>
      </MessageResponse>;
  }
  if (output.card) {
    return <MessageResponse>
        <CardView card={output.card as LookupCard} />
      </MessageResponse>;
  }
  // No card (it failed the answer contract): the core's text still stands alone.
  return <MessageResponse>
      <Box flexDirection="column">
        <Text>{output.text}</Text>
        {output.credit ? <Text dimColor>{output.credit}</Text> : null}
      </Box>
    </MessageResponse>;
}
