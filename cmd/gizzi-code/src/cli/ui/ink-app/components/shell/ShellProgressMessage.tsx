import React from 'react';
import stripAnsi from 'strip-ansi';
import { Ansi, Box, Text } from '../../ink';
import { Highlight } from '@/shared/util/highlight';
import { formatFileSize } from '../../utils/format';
import { MessageResponse } from '../MessageResponse';
import { OffscreenFreeze } from '../OffscreenFreeze';
import { ShellTimeDisplay } from './ShellTimeDisplay';
type Props = {
  output: string;
  fullOutput: string;
  elapsedTimeSeconds?: number;
  totalLines?: number;
  totalBytes?: number;
  timeoutMs?: number;
  taskId?: string;
  verbose: boolean;
};
export function ShellProgressMessage({
    output,
    fullOutput,
    elapsedTimeSeconds,
    totalLines,
    totalBytes,
    timeoutMs,
    verbose
}: Props) {
  const t1 = stripAnsi(fullOutput.trim());

  const strippedFullOutput = t1;
  const strippedOutput = stripAnsi(output.trim());
  const lines = strippedOutput.split("\n").filter(_temp);
  const t2 = verbose ? strippedFullOutput : lines.slice(-5).join("\n");

  const displayLines = t2;
  if (!lines.length) {
    const t3 = <Text dimColor={true}>Running… </Text>;

    const t4 = <MessageResponse><OffscreenFreeze>{t3}<ShellTimeDisplay elapsedTimeSeconds={elapsedTimeSeconds} timeoutMs={timeoutMs} /></OffscreenFreeze></MessageResponse>;

    return t4;
  }
  const extraLines = totalLines ? Math.max(0, totalLines - 5) : 0;
  let lineStatus = "";
  if (!verbose && totalBytes && totalLines) {
    lineStatus = `~${totalLines} lines`;
  } else {
    if (!verbose && extraLines > 0) {
      lineStatus = `+${extraLines} lines`;
    }
  }
  const t3 = verbose ? undefined : Math.min(5, lines.length);
  // Live output: a dim gutter per line, ✓/✕ lines and paths highlighted
  // (shared with `gizzi run`).
  const t4 = <Box flexDirection="column">{displayLines.split("\n").map((line, i) => <Text key={i}><Text dimColor={true}>{"│ "}</Text><Ansi>{Highlight.line(line)}</Ansi></Text>)}</Box>;

  const t5 = <Box height={t3} flexDirection="column" overflow="hidden">{t4}</Box>;

  const t6 = lineStatus ? <Text dimColor={true}>{lineStatus}</Text> : null;

  const t7 = <ShellTimeDisplay elapsedTimeSeconds={elapsedTimeSeconds} timeoutMs={timeoutMs} />;

  const t8 = totalBytes ? <Text dimColor={true}>{formatFileSize(totalBytes)}</Text> : null;

  const t9 = <Box flexDirection="row" gap={1}>{t6}{t7}{t8}</Box>;

  const t10 = <MessageResponse><OffscreenFreeze><Box flexDirection="column">{t5}{t9}</Box></OffscreenFreeze></MessageResponse>;

  return t10;
}
function _temp(line) {
  return line;
}
