import * as React from 'react';
import { useMemo } from 'react';
import { type Command, formatDescriptionWithSource } from '../../commands';
import { Box, Text } from '../../ink';
import { truncate } from '../../utils/format';
import { Select } from '../CustomSelect/select';
import { useTabHeaderFocus } from '../design-system/Tabs';
import { groupCommandsForHelp } from '../../utils/suggestions/commandSource';
type Props = {
  commands: Command[];
  maxHeight: number;
  columns: number;
  title: string;
  onCancel: () => void;
  emptyMessage?: string;
  /** Show the commands under their job groups (Session, Code, Workspace, Account, Settings). */
  grouped?: boolean;
};
export function Commands({
    commands,
    maxHeight,
    columns,
    title,
    onCancel,
    emptyMessage,
    grouped
}: Props) {
  const {
    headerFocused,
    focusHeader
  } = useTabHeaderFocus();
  const maxWidth = Math.max(1, columns - 10);
  const visibleCount = Math.max(1, Math.floor((maxHeight - 10) / 2));
  const seen = new Set();
  const t2 = cmd_0 => ({
        label: `/${cmd_0.name}`,
        value: cmd_0.name,
        description: truncate(formatDescriptionWithSource(cmd_0), maxWidth, true)
      });

  const t1 = commands.filter(cmd => {
      if (seen.has(cmd.name)) {
        return false;
      }
      seen.add(cmd.name);
      return true;
    }).sort(_temp).map(t2);

  // Grouped: a dimmed, unselectable header row before each job group.
  const options = grouped ? groupCommandsForHelp(commands).flatMap(({ group, commands: groupCommands }) => [{
      label: <Text bold={true}>{group === 'Other' ? 'More' : group}</Text>,
      value: `group:${group}`,
      disabled: true
    }, ...groupCommands.map(t2)]) : t1;
  const t2_2 = <Box flexDirection="column" paddingY={1}>{commands.length === 0 && emptyMessage ? <Text dimColor={true}>{emptyMessage}</Text> : <><Text>{title}</Text><Box marginTop={1}><Select options={options} visibleOptionCount={visibleCount} onCancel={onCancel} disableSelection={true} hideIndexes={true} layout="compact-vertical" onUpFromFirstItem={focusHeader} isDisabled={headerFocused} /></Box></>}</Box>;

  return t2_2;
}
function _temp(a, b) {
  return a.name.localeCompare(b.name);
}
