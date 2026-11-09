import React from 'react';
import { Box, color, Link, Text, useTheme } from '../../ink';
import type { CommandResultDisplay } from '../../types/command';
import { SandboxManager } from '../../utils/sandbox/sandbox-adapter';
import { Select } from '../CustomSelect/select';
import { useTabHeaderFocus } from '../design-system/Tabs';
type Props = {
  onComplete: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
};
type OverrideMode = 'open' | 'closed';
export function SandboxOverridesTab({
    onComplete
}: Props) {
  const isEnabled = SandboxManager.isSandboxingEnabled();
  const isLocked = SandboxManager.areSandboxSettingsLockedByPolicy();
  const currentAllowUnsandboxed = SandboxManager.areUnsandboxedCommandsAllowed();
  if (!isEnabled) {
    const t1 = <Box flexDirection="column" paddingY={1}><Text color="subtle">Sandbox is not enabled. Enable sandbox to configure override settings.</Text></Box>;

    return t1;
  }
  if (isLocked) {
    const t1 = <Text color="subtle">Override settings are managed by a higher-priority configuration and cannot be changed locally.</Text>;

    const t2 = <Box flexDirection="column" paddingY={1}>{t1}<Box marginTop={1}><Text dimColor={true}>Current setting:{" "}{currentAllowUnsandboxed ? "Allow unsandboxed fallback" : "Strict sandbox mode"}</Text></Box></Box>;

    return t2;
  }
  const t1 = <OverridesSelect onComplete={onComplete} currentMode={currentAllowUnsandboxed ? "open" : "closed"} />;

  return t1;
}

// Split so useTabHeaderFocus() only runs when the Select renders. Calling it
// above the early returns registers a down-arrow opt-in even when we return
// static text — pressing ↓ then blurs the header with no way back.
function OverridesSelect(t0) {
  const {
    onComplete,
    currentMode
  } = t0;
  const [theme] = useTheme();
  const {
    headerFocused,
    focusHeader
  } = useTabHeaderFocus();
  const t1 = color("success", theme)("(current)");

  const currentIndicator = t1;
  const t2 = currentMode === "open" ? `Allow unsandboxed fallback ${currentIndicator}` : "Allow unsandboxed fallback";
  const t3 = {
      label: t2,
      value: "open"
    };

  const t4 = currentMode === "closed" ? `Strict sandbox mode ${currentIndicator}` : "Strict sandbox mode";
  const t5 = {
      label: t4,
      value: "closed"
    };

  const t6 = [t3, t5];

  const options = t6;
  const t7 = async function handleSelect(value) {
      const mode = value as OverrideMode;
      await SandboxManager.setSandboxSettings({
        allowUnsandboxedCommands: mode === "open"
      });
      const message = mode === "open" ? "\u2713 Unsandboxed fallback allowed - commands can run outside sandbox when necessary" : "\u2713 Strict sandbox mode - all commands must run in sandbox or be excluded via the `excludedCommands` option";
      onComplete(message);
    };

  const handleSelect = t7;
  const t8 = <Box marginBottom={1}><Text bold={true}>Configure Overrides:</Text></Box>;

  const t9 = () => onComplete(undefined, {
      display: "skip"
    });

  const t10 = <Select options={options} onChange={handleSelect} onCancel={t9} onUpFromFirstItem={focusHeader} isDisabled={headerFocused} />;

  const t11 = <Text dimColor={true}><Text bold={true} dimColor={true}>Allow unsandboxed fallback:</Text>{" "}When a command fails due to sandbox restrictions, Gizzi can retry with dangerouslyDisableSandbox to run outside the sandbox (falling back to default permissions).</Text>;

  const t12 = <Text dimColor={true}><Text bold={true} dimColor={true}>Strict sandbox mode:</Text>{" "}All bash commands invoked by the model must run in the sandbox unless they are explicitly listed in excludedCommands.</Text>;

  const t13 = <Box flexDirection="column" marginTop={1} gap={1}>{t11}{t12}<Text dimColor={true}>Learn more:{" "}<Link url="https://docs.allternit.com/cli/permission-profiles">docs.gizziio.com/sandboxing#configure-sandboxing</Link></Text></Box>;

  const t14 = <Box flexDirection="column" paddingY={1}>{t8}{t10}{t13}</Box>;

  return t14;
}
