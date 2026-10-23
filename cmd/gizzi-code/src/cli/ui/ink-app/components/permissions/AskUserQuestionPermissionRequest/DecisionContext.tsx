import React from 'react';
import { Box, Text } from '../../../ink';
import type { Question } from '../../../tools/AskUserQuestionTool/AskUserQuestionTool';

const RISK_COLOR = { high: 'error', medium: 'warning', low: 'success' } as const;

// Decision trees: the risk level and facts under the question title.
export function DecisionContext({ question }: { question: Question }) {
  if (!question.risk && !question.context) return null;
  return <Box flexDirection="row" gap={1}>
      {question.risk && <Text color={RISK_COLOR[question.risk]} bold>{question.risk.toUpperCase()} RISK</Text>}
      {question.context && <Text color="inactive" wrap="wrap">{question.context}</Text>}
    </Box>;
}
