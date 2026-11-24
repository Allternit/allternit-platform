import * as React from 'react';
import { Suspense, useState } from 'react';
import { useKeybinding } from '../../keybindings/useKeybinding';
import { useExitOnCtrlCDWithKeybindings } from '../../hooks/useExitOnCtrlCDWithKeybindings';
import { useTerminalSize } from '../../hooks/useTerminalSize';
import { useIsInsideModal, useModalOrTerminalSize } from '../../context/modalContext';
import { Pane } from '../design-system/Pane';
import { BlockTitle } from '../BlockTitle';
import { Tabs, Tab } from '../design-system/Tabs';
import { Status, buildDiagnostics } from './Status';
import { Config } from './Config';
import { Usage } from './Usage';
import type { LocalJSXCommandContext, CommandResultDisplay } from '../../commands';
type Props = {
  onClose: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
  context: LocalJSXCommandContext;
  defaultTab: 'Status' | 'Config' | 'Usage' | 'Gates';
};
export function Settings({
    onClose,
    context,
    defaultTab
}: Props) {
  const [selectedTab, setSelectedTab] = useState<string>(defaultTab);
  const [tabsHidden, setTabsHidden] = useState(false);
  const [configOwnsEsc, setConfigOwnsEsc] = useState(false);
  const [gatesOwnsEsc, setGatesOwnsEsc] = useState(false);
  const insideModal = useIsInsideModal();
  const {
    rows
  } = useModalOrTerminalSize(useTerminalSize());
  const contentHeight = insideModal ? rows + 1 : Math.max(15, Math.min(Math.floor(rows * 0.8), 30));
  const [diagnosticsPromise] = useState(_temp2);
  useExitOnCtrlCDWithKeybindings();
  const t1 = () => {
      if (tabsHidden) {
        return;
      }
      onClose("Status dialog dismissed", {
        display: "system"
      });
    };

  const handleEscape = t1;
  const t2 = !tabsHidden && !(selectedTab === "Config" && configOwnsEsc) && !(selectedTab === "Gates" && gatesOwnsEsc);
  const t3 = {
      context: "Settings",
      isActive: t2
    };

  useKeybinding("confirm:no", handleEscape, t3);
  let t4;
  t4 = <Tab key="status" title="Status"><Status context={context} diagnosticsPromise={diagnosticsPromise} isActiveTab={selectedTab === "Status"} /></Tab>;
  const t5 = <Tab key="config" title="Config"><Suspense fallback={null}><Config context={context} onClose={onClose} setTabsHidden={setTabsHidden} onIsSearchModeChange={setConfigOwnsEsc} contentHeight={contentHeight} /></Suspense></Tab>;

  const t6 = <Tab key="usage" title="Usage"><Usage /></Tab>;

  // @ts-expect-error TODO(types) — Gates is ant-only upstream; dead branch
  const t7 = false ? [<Tab key="gates" title="Gates"><Gates onOwnsEscChange={setGatesOwnsEsc} contentHeight={contentHeight} /></Tab>] : [];

  const t8 = [t4, t5, t6, ...t7];

  const tabs = t8;
  const t9 = defaultTab !== "Config" && defaultTab !== "Gates";
  const t10 = tabsHidden || insideModal ? undefined : contentHeight;
  const t11 = <Pane color="permission"><BlockTitle title="SETTINGS" /><Tabs color="permission" selectedTab={selectedTab} onTabChange={setSelectedTab} hidden={tabsHidden} initialHeaderFocused={t9} contentHeight={t10}>{tabs}</Tabs></Pane>;

  return t11;
}
function _temp2() {
  return buildDiagnostics().catch(_temp);
}
function _temp() {
  return [];
}
