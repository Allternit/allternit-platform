import React, { type ReactNode } from 'react';
import { Box } from '../../../../ink';
import { useKeybinding } from '../../../../keybindings/useKeybinding';
import { isAutoMemoryEnabled } from '../../../../memdir/paths';
import { type AgentMemoryScope, loadAgentMemoryPrompt } from '../../../../tools/AgentTool/agentMemory';
import { ConfigurableShortcutHint } from '../../../ConfigurableShortcutHint';
import { Select } from '../../../CustomSelect/select';
import { Byline } from '../../../design-system/Byline';
import { KeyboardShortcutHint } from '../../../design-system/KeyboardShortcutHint';
import { useWizard } from '../../../wizard/index';
import { WizardDialogLayout } from '../../../wizard/WizardDialogLayout';
import type { AgentWizardData } from '../types';
type MemoryOption = {
  label: string;
  value: AgentMemoryScope | 'none';
};
export function MemoryStep() {
  const {
    goNext,
    goBack,
    updateWizardData,
    wizardData
  } = useWizard();
  const t0 = {
      context: "Confirmation"
    };

  useKeybinding("confirm:no", goBack, t0);
  const isUserScope = wizardData.location === "userSettings";
  const t1 = isUserScope ? [{
      label: "User scope (~/.gizzi/agent-memory/) (Recommended)",
      value: "user"
    }, {
      label: "None (no persistent memory)",
      value: "none"
    }, {
      label: "Project scope (.claude/agent-memory/)",
      value: "project"
    }, {
      label: "Local scope (.claude/agent-memory-local/)",
      value: "local"
    }] : [{
      label: "Project scope (.claude/agent-memory/) (Recommended)",
      value: "project"
    }, {
      label: "None (no persistent memory)",
      value: "none"
    }, {
      label: "User scope (~/.gizzi/agent-memory/)",
      value: "user"
    }, {
      label: "Local scope (.claude/agent-memory-local/)",
      value: "local"
    }];

  const memoryOptions = t1;
  const t2 = value => {
      const memory = value === "none" ? undefined : value as AgentMemoryScope;
      const agentType = wizardData.finalAgent?.agentType;
      updateWizardData({
        selectedMemory: memory,
        finalAgent: wizardData.finalAgent ? {
          ...wizardData.finalAgent,
          memory,
          getSystemPrompt: isAutoMemoryEnabled() && memory && agentType ? () => wizardData.systemPrompt + "\n\n" + loadAgentMemoryPrompt(agentType, memory) : () => wizardData.systemPrompt
        } : undefined
      });
      goNext();
    };

  const handleSelect = t2;
  const t3 = <Byline><KeyboardShortcutHint shortcut={"\u2191\u2193"} action="navigate" /><KeyboardShortcutHint shortcut="Enter" action="select" /><ConfigurableShortcutHint action="confirm:no" context="Confirmation" fallback="Esc" description="go back" /></Byline>;

  const t4 = <WizardDialogLayout subtitle="Configure agent memory" footerText={t3}><Box><Select key="memory-select" options={memoryOptions} onChange={handleSelect} onCancel={goBack} /></Box></WizardDialogLayout>;

  return t4;
}
