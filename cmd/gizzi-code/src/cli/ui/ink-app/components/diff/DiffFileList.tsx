import figures from 'figures';
import React, { useMemo } from 'react';
import type { DiffFile } from '../../hooks/useDiffData';
import { useTerminalSize } from '../../hooks/useTerminalSize';
import { Box, Text } from '../../ink';
import { truncateStartToWidth } from '../../utils/format';
import { plural } from '../../utils/stringUtils';
import { EmptyLine } from '../EmptyLine'
const MAX_VISIBLE_FILES = 5;
type Props = {
  files: DiffFile[];
  selectedIndex: number;
};
export function DiffFileList({
    files,
    selectedIndex
}: Props) {
  const {
    columns
  } = useTerminalSize();
  let t1;
  bb0: {
    if (files.length === 0 || files.length <= MAX_VISIBLE_FILES) {
      const t2 = {
          startIndex: 0,
          endIndex: files.length
        };

      t1 = t2;
      break bb0;
    }
    let start = Math.max(0, selectedIndex - Math.floor(MAX_VISIBLE_FILES / 2));
    let end = start + MAX_VISIBLE_FILES;
    if (end > files.length) {
      end = files.length;
      start = Math.max(0, end - MAX_VISIBLE_FILES);
    }
    const t2 = {
        startIndex: start,
        endIndex: end
      };

    t1 = t2;
  }
  const {
    startIndex,
    endIndex
  } = t1;
  if (files.length === 0) {
    const t2 = <EmptyLine>No changed files</EmptyLine>;

    return t2;
  }
  let T0;
  let hasMoreBelow;
  let needsPagination;
  let t2;
  let t3;
  let t4;
  const visibleFiles = files.slice(startIndex, endIndex);
  const hasMoreAbove = startIndex > 0;
  hasMoreBelow = endIndex < files.length;
  needsPagination = files.length > MAX_VISIBLE_FILES;
  const maxPathWidth = Math.max(20, columns - 16 - 3 - 4);
  T0 = Box;
  t2 = "column";

    t3 = needsPagination && <Text dimColor={true}>{hasMoreAbove ? ` ↑ ${startIndex} more ${plural(startIndex, "file")}` : " "}</Text>;
  
  const t5 = (file, index) => <FileItem key={file.path} file={file} isSelected={startIndex + index === selectedIndex} maxPathWidth={maxPathWidth} />;

  t4 = visibleFiles.map(t5);
  

  const t5_2 = needsPagination && <Text dimColor={true}>{hasMoreBelow ? ` ↓ ${files.length - endIndex} more ${plural(files.length - endIndex, "file")}` : " "}</Text>;

  const t6 = <T0 flexDirection={t2}>{t3}{t4}{t5_2}</T0>;

  return t6;
}
function FileItem(t0) {
  const {
    file,
    isSelected,
    maxPathWidth
  } = t0;
  const t1 = truncateStartToWidth(file.path, maxPathWidth);

  const displayPath = t1;
  const pointer = isSelected ? figures.pointer + " " : "  ";
  const line = `${pointer}${displayPath}`;
  const t2 = isSelected ? "background" : undefined;
  const t3 = <Text bold={isSelected} color={t2} inverse={isSelected}>{line}</Text>;

  const t4 = <Box flexGrow={1} />;

  const t5 = <FileStats file={file} isSelected={isSelected} />;

  const t6 = <Box flexDirection="row">{t3}{t4}{t5}</Box>;

  return t6;
}
function FileStats(t0) {
  const {
    file,
    isSelected
  } = t0;
  if (file.isUntracked) {
    const t1 = !isSelected;
    const t2 = <Text dimColor={t1} italic={true}>untracked</Text>;

    return t2;
  }
  if (file.isBinary) {
    const t1 = !isSelected;
    const t2 = <Text dimColor={t1} italic={true}>Binary file</Text>;

    return t2;
  }
  if (file.isLargeFile) {
    const t1 = !isSelected;
    const t2 = <Text dimColor={t1} italic={true}>Large file modified</Text>;

    return t2;
  }
  const t1 = file.linesAdded > 0 && <Text color="diffAddedWord" bold={isSelected}>+{file.linesAdded}</Text>;

  const t2 = file.linesAdded > 0 && file.linesRemoved > 0 && " ";
  const t3 = file.linesRemoved > 0 && <Text color="diffRemovedWord" bold={isSelected}>-{file.linesRemoved}</Text>;

  const t4 = file.isTruncated && <Text dimColor={!isSelected}> (truncated)</Text>;

  const t5 = <Text>{t1}{t2}{t3}{t4}</Text>;

  return t5;
}
