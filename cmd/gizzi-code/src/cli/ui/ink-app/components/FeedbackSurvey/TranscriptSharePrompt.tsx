import React from 'react';
import { BLACK_CIRCLE } from '../../constants/figures';
import { Box, Text } from '../../ink';
import { useDebouncedDigitInput } from './useDebouncedDigitInput';
export type TranscriptShareResponse = 'yes' | 'no' | 'dont_ask_again';
type Props = {
  onSelect: (option: TranscriptShareResponse) => void;
  inputValue: string;
  setInputValue: (value: string) => void;
};
const RESPONSE_INPUTS = ['1', '2', '3'] as const;
type ResponseInput = (typeof RESPONSE_INPUTS)[number];
const inputToResponse: Record<ResponseInput, TranscriptShareResponse> = {
  '1': 'yes',
  '2': 'no',
  '3': 'dont_ask_again'
} as const;
const isValidResponseInput = (input: string): input is ResponseInput => (RESPONSE_INPUTS as readonly string[]).includes(input);
export function TranscriptSharePrompt({
    onSelect,
    inputValue,
    setInputValue
}: Props) {
  const t1 = digit => onSelect(inputToResponse[digit]);

  const t2 = {
      inputValue,
      setInputValue,
      isValidDigit: isValidResponseInput,
      onDigit: t1
    };

  useDebouncedDigitInput(t2);
  const t3 = <Box><Text color="ansi:cyan">{BLACK_CIRCLE} </Text><Text bold={true}>Can the Gizzi team look at your session transcript to help us improve Gizzi Code?</Text></Box>;

  const t4 = <Box marginLeft={2}><Text dimColor={true}>Learn more: https://docs.allternit.com/security/overview#data-protection</Text></Box>;

  const t5 = <Box width={10}><Text><Text color="ansi:cyan">1</Text>: Yes</Text></Box>;

  const t6 = <Box width={10}><Text><Text color="ansi:cyan">2</Text>: No</Text></Box>;

  const t7 = <Box flexDirection="column" marginTop={1}>{t3}{t4}<Box marginLeft={2}>{t5}{t6}<Box><Text><Text color="ansi:cyan">3</Text>: Don't ask again</Text></Box></Box></Box>;

  return t7;
}
