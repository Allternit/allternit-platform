import * as React from 'react';
import { useState } from 'react';
import { useKeybinding } from '../../keybindings/useKeybinding';
import { useExitOnCtrlCDWithKeybindings } from '../../hooks/useExitOnCtrlCDWithKeybindings';
import { useTerminalSize } from '../../hooks/useTerminalSize';
import { useIsInsideModal, useModalOrTerminalSize } from '../../context/modalContext';
import { Box, Text } from '../../ink';
import { Pane } from '../../components/design-system/Pane';
import { BlockTitle } from '../../components/BlockTitle';
import { Tabs, Tab } from '../../components/design-system/Tabs';
import { Status, buildDiagnostics } from '../../components/Settings/Status';
import { Usage } from '../../components/Settings/Usage';
import { Stats } from '../../components/Stats';
import type { LocalJSXCommandContext, CommandResultDisplay } from '../../commands';
import { costSummaryText } from '../cost/cost';
import type { StatusTab } from './tabs';

type Props = {
  onClose: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
  context: LocalJSXCommandContext;
  defaultTab: StatusTab;
  /** Shown above the Usage tab, e.g. the result of /extra-usage. */
  usageNotice?: string;
};

/**
 * /status: Status, Usage, Cost and Stats as tabs (Tab or ←/→ switches).
 * Each tab reuses the component its old command rendered.
 */
export function StatusPanel({ onClose, context, defaultTab, usageNotice }: Props): React.ReactNode {
  const [selectedTab, setSelectedTab] = useState<string>(defaultTab);
  const insideModal = useIsInsideModal();
  const { rows } = useModalOrTerminalSize(useTerminalSize());
  const contentHeight = insideModal ? rows + 1 : Math.max(15, Math.min(Math.floor(rows * 0.8), 30));
  const [diagnosticsPromise] = useState(() => buildDiagnostics().catch(() => []));
  useExitOnCtrlCDWithKeybindings();
  useKeybinding('confirm:no', () => onClose('Status dialog dismissed', { display: 'system' }), {
    context: 'Settings',
  });
  return <Pane color="permission">
      <BlockTitle title="STATUS" />
      <Tabs color="permission" selectedTab={selectedTab} onTabChange={setSelectedTab} contentHeight={insideModal ? undefined : contentHeight}>
        <Tab key="status" title="Status">
          <Status context={context} diagnosticsPromise={diagnosticsPromise} isActiveTab={selectedTab === 'Status'} />
        </Tab>
        <Tab key="usage" title="Usage">
          <Box flexDirection="column" gap={1} width="100%">
            {usageNotice ? <Text>{usageNotice}</Text> : null}
            <Usage />
          </Box>
        </Tab>
        <Tab key="cost" title="Cost">
          <Text>{costSummaryText()}</Text>
        </Tab>
        <Tab key="stats" title="Stats">
          <Stats onClose={onClose} embedded />
        </Tab>
      </Tabs>
    </Pane>;
}
