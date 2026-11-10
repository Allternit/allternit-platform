import type { StructuredPatchHunk } from 'diff';
import * as React from 'react';
import { useTerminalSize } from '../hooks/useTerminalSize';
import { Box, Text } from '../ink';
import { count } from '../utils/array';
import { MessageResponse } from './MessageResponse';
import { StructuredDiffList } from './StructuredDiffList';
import { EditDiagnosticsLine } from './EditDiagnosticsLine';
type Props = {
  filePath: string;
  structuredPatch: StructuredPatchHunk[];
  firstLine: string | null;
  fileContent?: string;
  style?: 'condensed';
  verbose: boolean;
  previewHint?: string;
  /** Language-server errors for this edit (services/lsp/editDiagnostics). */
  lspKey?: string;
};
export function FileEditToolUpdatedMessage({
    filePath,
    structuredPatch,
    firstLine,
    fileContent,
    style,
    verbose,
    previewHint,
    lspKey
}: Props) {
  const {
    columns
  } = useTerminalSize();
  const numAdditions = structuredPatch.reduce(_temp2, 0);
  const numRemovals = structuredPatch.reduce(_temp4, 0);
  const t1 = numAdditions > 0 ? <>Added <Text bold={true}>{numAdditions}</Text>{" "}{numAdditions > 1 ? "lines" : "line"}</> : null;

  const t2 = numAdditions > 0 && numRemovals > 0 ? ", " : null;
  const t3 = numRemovals > 0 ? <>{numAdditions === 0 ? "R" : "r"}emoved <Text bold={true}>{numRemovals}</Text>{" "}{numRemovals > 1 ? "lines" : "line"}</> : null;

  const t4 = <Text>{t1}{t2}{t3}</Text>;

  const text = t4;
  if (previewHint) {
    if (style !== "condensed" && !verbose) {
      const t5 = <MessageResponse><Text dimColor={true}>{previewHint}</Text></MessageResponse>;

      return t5;
    }
  } else {
    if (style === "condensed" && !verbose) {
      return lspKey ? <Box flexDirection="column">{text}<EditDiagnosticsLine lspKey={lspKey} /></Box> : text;
    }
  }
  const t5 = <Text>{text}</Text>;

  const t6 = columns - 12;
  const t7 = <StructuredDiffList hunks={structuredPatch} dim={false} width={t6} filePath={filePath} firstLine={firstLine} fileContent={fileContent} />;

  const t8 = <MessageResponse><Box flexDirection="column">{t5}{lspKey ? <EditDiagnosticsLine lspKey={lspKey} /> : null}{t7}</Box></MessageResponse>;

  return t8;
}
function _temp4(acc_0, hunk_0) {
  return acc_0 + count(hunk_0.lines, _temp3);
}
function _temp3(__0) {
  return __0.startsWith("-");
}
function _temp2(acc, hunk) {
  return acc + count(hunk.lines, _temp);
}
function _temp(_) {
  return _.startsWith("+");
}
