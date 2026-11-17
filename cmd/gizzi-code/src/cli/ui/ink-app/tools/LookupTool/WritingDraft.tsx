/**
 * The writing answer card in the terminal: a fenced `openui` block whose JSON
 * is a valid `writing` envelope (answer-contract WRITING_EXAMPLE) draws as a
 * bordered draft (To / Subject / body) with its alternatives listed below.
 * Anything else in an `openui` fence stays an ordinary code block.
 *
 * Nothing here sends: the user copies the draft and sends it themselves.
 */
import React from 'react';
import { parseAnswerEnvelope } from '@/runtime/integrations/artifacts/answer-contract';
import { Box, Text } from '../../ink';

export type WritingDraftData = {
  variant: 'email' | 'text_message' | 'social_post' | 'document' | 'note';
  to?: string;
  subject?: string;
  body: string;
  alternatives?: Array<{ label: string; body: string }>;
};
export type WritingDraftCard = { title: string; summary?: string; data: WritingDraftData };

const VARIANT_LABEL: Record<WritingDraftData['variant'], string> = {
  email: 'Email',
  text_message: 'Text message',
  social_post: 'Social post',
  document: 'Document',
  note: 'Note',
};

/** The writing envelope in an `openui` fence's code, or undefined (not JSON, another template, or invalid). */
export function parseWritingDraft(lang: string | undefined, code: string): WritingDraftCard | undefined {
  if (lang?.trim().toLowerCase() !== 'openui' || !code.includes('"writing"')) return undefined;
  const { answer } = parseAnswerEnvelope(code);
  if (!answer || answer.template !== 'writing') return undefined;
  return answer as unknown as WritingDraftCard;
}

/** The readable text /copy puts on the clipboard: To and Subject lines, the body, then each alternative under its label. */
export function writingCopyText(card: WritingDraftCard): string {
  const { to, subject, body, alternatives } = card.data;
  const head = [...(to ? [`To: ${to}`] : []), ...(subject ? [`Subject: ${subject}`] : [])];
  const parts = [head.length ? `${head.join('\n')}\n\n${body}` : body];
  for (const alt of alternatives ?? []) parts.push(`Alternative (${alt.label}):\n${alt.body}`);
  return parts.join('\n\n');
}

export function WritingDraft({ card }: { card: WritingDraftCard }): React.ReactNode {
  const { variant, to, subject, body, alternatives } = card.data;
  return <Box flexDirection="column">
      <Box flexDirection="column" borderStyle="round" borderDimColor paddingX={1}>
        <Text>
          <Text bold>{card.title}</Text>
          <Text dimColor> · {VARIANT_LABEL[variant]}</Text>
        </Text>
        {to ? <Text><Text dimColor>To       </Text>{to}</Text> : null}
        {subject ? <Text><Text dimColor>Subject  </Text>{subject}</Text> : null}
        <Box marginTop={1}>
          <Text>{body}</Text>
        </Box>
      </Box>
      {alternatives?.map((alt, i) => <Box key={i} flexDirection="column" paddingLeft={2} marginTop={1}>
          <Text dimColor>Alternative: {alt.label}</Text>
          <Text>{alt.body}</Text>
        </Box>)}
      <Text dimColor>Draft only, not sent · /copy to copy it as text</Text>
    </Box>;
}
