import figures from 'figures';
import React from 'react';
import { Box, Text } from '../../ink';
import type { Output } from './AskUserLaterTool';

type Question = {
  question: string;
  header?: string;
  multiple?: boolean;
  options: Array<{ label: string; description?: string; recommended?: boolean; caution?: string; then?: { question: string; options: Array<{ label: string }> } }>;
};

export function renderToolUseMessage(input: { questions?: Question[] }): React.ReactNode {
  const n = input.questions?.length ?? 0;
  return input.questions?.[0]?.header || (n ? `${n} question${n === 1 ? '' : 's'}` : '');
}

const PUSH_LABEL: Record<Output['push'], string | null> = {
  sent: 'Sent to your phone',
  not_requested: null,
  rate_limited: 'Not pushed: too many recently',
  unpaired: 'Not pushed: this runtime is not linked to your cloud account',
  unavailable: null,
};

export function renderToolResultMessage(output: Output): React.ReactNode {
  const questions = output.questions as Question[];
  const push = PUSH_LABEL[output.push];
  return <Box flexDirection="row" marginTop={1}>
      <Box minWidth={2} />
      <Box flexDirection="column">
        {questions.map((q, i) => <Box key={i} flexDirection="column" marginBottom={1}>
            <Text bold>{q.question}{q.multiple ? <Text dimColor> (pick any)</Text> : null}</Text>
            {q.options.map((o, j) => <Box key={j} flexDirection="column">
                <Text>
                  {'  '}{j + 1}. {o.label}
                  {o.recommended ? <Text color="success"> (recommended)</Text> : null}
                  {o.description ? <Text dimColor> {figures.line} {o.description}</Text> : null}
                </Text>
                {o.caution ? <Text color="warning">{'     '}{o.caution}</Text> : null}
                {o.then ? <Text dimColor>{'     '}then: {o.then.question} ({o.then.options.map(f => f.label).join(' / ')})</Text> : null}
              </Box>)}
          </Box>)}
        <Text>
          <Text dimColor>Assuming until you answer: </Text>
          {output.assumption}
        </Text>
        <Text dimColor>
          Answer later with /answer {output.ask_id}
          {output.live ? ', or from the Allternit app' : ''}
          {push ? ` · ${push}` : ''}
        </Text>
      </Box>
    </Box>;
}
