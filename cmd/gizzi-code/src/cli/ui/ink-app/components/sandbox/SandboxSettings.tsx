import React from 'react';
import { Box, color, Link, Text, useTheme } from '../../ink';
import { useKeybindings } from '../../keybindings/useKeybinding';
import type { CommandResultDisplay } from '../../types/command';
import type { SandboxDependencyCheck } from '../../utils/sandbox/sandbox-adapter';
import { SandboxManager } from '../../utils/sandbox/sandbox-adapter';
import { getSettings_DEPRECATED } from '../../utils/settings/settings';
import { Select } from '../CustomSelect/select';
import { Pane } from '../design-system/Pane';
import { Tab, Tabs, useTabHeaderFocus } from '../design-system/Tabs';
import { SandboxConfigTab } from './SandboxConfigTab';
import { SandboxDependenciesTab } from './SandboxDependenciesTab';
import { SandboxOverridesTab } from './SandboxOverridesTab';
type Props = {
  onComplete: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
  depCheck: SandboxDependencyCheck;
};
type SandboxMode = 'auto-allow' | 'regular' | 'disabled';
export function SandboxSettings({
    onComplete,
    depCheck
}: Props) {
  const [theme] = useTheme();
  const currentEnabled = SandboxManager.isSandboxingEnabled();
  const currentAutoAllow = SandboxManager.isAutoAllowBashIfSandboxedEnabled();
  const hasWarnings = depCheck.warnings.length > 0;
  const t1 = getSettings_DEPRECATED();

  const settings = t1;
  const allowAllUnixSockets = settings.sandbox?.network?.allowAllUnixSockets;
  const showSocketWarning = hasWarnings && !allowAllUnixSockets;
  const getCurrentMode = () => {
    if (!currentEnabled) {
      return "disabled";
    }
    if (currentAutoAllow) {
      return "auto-allow";
    }
    return "regular";
  };
  const currentMode = getCurrentMode();
  const t2 = color("success", theme)("(current)");

  const currentIndicator = t2;
  const t3 = currentMode === "auto-allow" ? `Sandbox BashTool, with auto-allow ${currentIndicator}` : "Sandbox BashTool, with auto-allow";
  const t4 = {
      label: t3,
      value: "auto-allow"
    };

  const t5 = currentMode === "regular" ? `Sandbox BashTool, with regular permissions ${currentIndicator}` : "Sandbox BashTool, with regular permissions";
  const t6 = {
      label: t5,
      value: "regular"
    };

  const t7 = currentMode === "disabled" ? `No Sandbox ${currentIndicator}` : "No Sandbox";
  const t8 = {
      label: t7,
      value: "disabled"
    };

  const t9 = [t4, t6, t8];

  const options = t9;
  const t10 = async function handleSelect(value) {
      const mode = value as SandboxMode;
      bb33: switch (mode) {
        case "auto-allow":
          {
            await SandboxManager.setSandboxSettings({
              enabled: true,
              autoAllowBashIfSandboxed: true
            });
            onComplete("\u2713 Sandbox enabled with auto-allow for bash commands");
            break bb33;
          }
        case "regular":
          {
            await SandboxManager.setSandboxSettings({
              enabled: true,
              autoAllowBashIfSandboxed: false
            });
            onComplete("\u2713 Sandbox enabled with regular bash permissions");
            break bb33;
          }
        case "disabled":
          {
            await SandboxManager.setSandboxSettings({
              enabled: false,
              autoAllowBashIfSandboxed: false
            });
            onComplete("\u25CB Sandbox disabled");
          }
      }
    };

  const handleSelect = t10;
  const t11 = {
      "confirm:no": () => onComplete(undefined, {
        display: "skip"
      })
    };

  const t12 = {
      context: "Settings"
    };

  useKeybindings(t11, t12);
  const t13 = <Tab key="mode" title="Mode"><SandboxModeTab showSocketWarning={showSocketWarning} options={options} onSelect={handleSelect} onComplete={onComplete} /></Tab>;

  const modeTab = t13;
  const t14 = <Tab key="overrides" title="Overrides"><SandboxOverridesTab onComplete={onComplete} /></Tab>;

  const overridesTab = t14;
  const t15 = <Tab key="config" title="Config"><SandboxConfigTab /></Tab>;

  const configTab = t15;
  const hasErrors = depCheck.errors.length > 0;
  const t16 = hasErrors ? [<Tab key="dependencies" title="Dependencies"><SandboxDependenciesTab depCheck={depCheck} /></Tab>] : [modeTab, ...(hasWarnings ? [<Tab key="dependencies" title="Dependencies"><SandboxDependenciesTab depCheck={depCheck} /></Tab>] : []), overridesTab, configTab];

  const tabs = t16;
  const t17 = <Pane color="permission"><Tabs title="Sandbox:" color="permission" defaultTab="Mode">{tabs}</Tabs></Pane>;

  return t17;
}
function SandboxModeTab(t0) {
  const {
    showSocketWarning,
    options,
    onSelect,
    onComplete
  } = t0;
  const {
    headerFocused,
    focusHeader
  } = useTabHeaderFocus();
  const t1 = showSocketWarning && <Box marginBottom={1}><Text color="warning">Cannot block unix domain sockets (see Dependencies tab)</Text></Box>;

  const t2 = <Box marginBottom={1}><Text bold={true}>Configure Mode:</Text></Box>;

  const t3 = () => onComplete(undefined, {
      display: "skip"
    });

  const t4 = <Select options={options} onChange={onSelect} onCancel={t3} onUpFromFirstItem={focusHeader} isDisabled={headerFocused} />;

  const t5 = <Text dimColor={true}><Text bold={true} dimColor={true}>Auto-allow mode:</Text>{" "}Commands will try to run in the sandbox automatically, and attempts to run outside of the sandbox fallback to regular permissions. Explicit ask/deny rules are always respected.</Text>;

  const t6 = <Box flexDirection="column" marginTop={1} gap={1}>{t5}<Text dimColor={true}>Learn more:{" "}<Link url="https://docs.allternit.com/cli/permission-profiles">docs.gizziio.com/sandboxing</Link></Text></Box>;

  const t7 = <Box flexDirection="column" paddingY={1}>{t1}{t2}{t4}{t6}</Box>;

  return t7;
}
