import React, { type ReactNode } from 'react';
import { Box } from '../../../../ink';
import type { SettingSource } from '../../../../utils/settings/constants';
import { ConfigurableShortcutHint } from '../../../ConfigurableShortcutHint';
import { Select } from '../../../CustomSelect/select';
import { Byline } from '../../../design-system/Byline';
import { KeyboardShortcutHint } from '../../../design-system/KeyboardShortcutHint';
import { useWizard } from '../../../wizard/index';
import { WizardDialogLayout } from '../../../wizard/WizardDialogLayout';
import type { AgentWizardData } from '../types';
export function LocationStep() {
  const {
    goNext,
    updateWizardData,
    cancel
  } = useWizard();
  const t0 = {
      label: "Project (.claude/agents/)",
      value: "projectSettings" as SettingSource
    };

  const t1 = [t0, {
      label: "Personal (~/.gizzi/agents/)",
      value: "userSettings" as SettingSource
    }];

  const locationOptions = t1;
  const t2 = <Byline><KeyboardShortcutHint shortcut={"\u2191\u2193"} action="navigate" /><KeyboardShortcutHint shortcut="Enter" action="select" /><ConfigurableShortcutHint action="confirm:no" context="Confirmation" fallback="Esc" description="cancel" /></Byline>;

  const t3 = value => {
      updateWizardData({
        location: value as SettingSource
      });
      goNext();
    };

  const t4 = () => cancel();

  const t5 = <WizardDialogLayout subtitle="Choose location" footerText={t2}><Box><Select key="location-select" options={locationOptions} onChange={t3} onCancel={t4} /></Box></WizardDialogLayout>;

  return t5;
}
