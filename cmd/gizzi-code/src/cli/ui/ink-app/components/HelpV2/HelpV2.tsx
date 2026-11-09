import * as React from 'react';
import { useExitOnCtrlCDWithKeybindings } from './../../hooks/useExitOnCtrlCDWithKeybindings.ts';
import { useShortcutDisplay } from './../../keybindings/useShortcutDisplay.ts';
import { builtInCommandNames, type Command, type CommandResultDisplay, INTERNAL_ONLY_COMMANDS } from '../../commands';
import { useIsInsideModal } from '../../context/modalContext';
import { useTerminalSize } from '../../hooks/useTerminalSize';
import { Box, Link, Text } from '../../ink';
import { useKeybinding } from '../../keybindings/useKeybinding';
import { Pane } from '../design-system/Pane';
import { Tab, Tabs } from '../design-system/Tabs';
import { Commands } from './Commands';
import { General } from './General';
type Props = {
  onClose: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
  commands: Command[];
};
export function HelpV2({
    onClose,
    commands
}: Props) {
  const {
    rows,
    columns
  } = useTerminalSize();
  const maxHeight = Math.floor(rows / 2);
  const insideModal = useIsInsideModal();
  const t1 = () => onClose("Help dialog dismissed", {
      display: "system"
    });

  const close = t1;
  const t2 = {
      context: "Help"
    };

  useKeybinding("help:dismiss", close, t2);
  const exitState = useExitOnCtrlCDWithKeybindings(close);
  const dismissShortcut = useShortcutDisplay("help:dismiss", "Help", "esc");
  const builtinNames = builtInCommandNames();
  const builtinCommands = commands.filter(cmd => builtinNames.has(cmd.name) && !cmd.isHidden);
  const t4 = [];

  const antOnlyCommands = t4;
  const t3 = commands.filter(cmd_2 => !builtinNames.has(cmd_2.name) && !cmd_2.isHidden);

  const customCommands = t3;
  const t4_2 = <Tab key="general" title="general"><General /></Tab>;

  const tabs = [t4_2];
  const t5 = <Tab key="commands" title="commands"><Commands commands={builtinCommands} maxHeight={maxHeight} columns={columns} title="Browse default commands:" onCancel={close} /></Tab>;

  tabs.push(t5);
  const t6 = <Tab key="custom" title="custom-commands"><Commands commands={customCommands} maxHeight={maxHeight} columns={columns} title="Browse custom commands:" emptyMessage="No custom commands found" onCancel={close} /></Tab>;

  tabs.push(t6);
  if (false && antOnlyCommands.length > 0) {
      const t7 = <Tab key="ant-only" title="[ant-only]"><Commands commands={antOnlyCommands} maxHeight={maxHeight} columns={columns} title="Browse ant-only commands:" onCancel={close} /></Tab>;

      tabs.push(t7);
    }

  const t5_2 = insideModal ? undefined : maxHeight;
  const t6_2 = <Tabs title={false ? "/help" : `Gizzi Code v${MACRO.VERSION}`} color="professionalBlue" defaultTab="general">{tabs}</Tabs>;

  const t7 = <Box marginTop={1}><Text>For more help:{" "}<Link url="https://docs.allternit.com/cli/overview" /></Text></Box>;

  const t8 = <Box marginTop={1}><Text dimColor={true}>{exitState.pending ? <>Press {exitState.keyName} again to exit</> : <Text italic={true}>{dismissShortcut} to cancel</Text>}</Text></Box>;

  const t9 = <Pane color="professionalBlue">{t6_2}{t7}{t8}</Pane>;

  const t10 = <Box flexDirection="column" height={t5_2}>{t9}</Box>;

  return t10;
}
