import * as React from 'react';
import { useCallback } from 'react';
import { Select } from '../../../components/CustomSelect/select';
import { Box, Text } from '../../../ink';
import type { ToolPermissionContext } from '../../../Tool';
import type { PermissionBehavior, PermissionRule, PermissionRuleValue } from '../../../utils/permissions/PermissionRule';
import { applyPermissionUpdate, persistPermissionUpdate } from '../../../utils/permissions/PermissionUpdate';
import { permissionRuleValueToString } from '../../../utils/permissions/permissionRuleParser';
import { detectUnreachableRules, type UnreachableRule } from '../../../utils/permissions/shadowedRuleDetection';
import { SandboxManager } from '../../../utils/sandbox/sandbox-adapter';
import { type EditableSettingSource, SOURCES } from '../../../utils/settings/constants';
import { getRelativeSettingsFilePathForSource } from '../../../utils/settings/settings';
import { plural } from '../../../utils/stringUtils';
import type { OptionWithDescription } from '../../CustomSelect/select';
import { Dialog } from '../../design-system/Dialog';
import { PermissionRuleDescription } from './PermissionRuleDescription';
export function optionForPermissionSaveDestination(saveDestination: EditableSettingSource): OptionWithDescription {
  switch (saveDestination) {
    case 'localSettings':
      return {
        label: 'Project settings (local)',
        description: `Saved in ${getRelativeSettingsFilePathForSource('localSettings')}`,
        value: saveDestination
      };
    case 'projectSettings':
      return {
        label: 'Project settings',
        description: `Checked in at ${getRelativeSettingsFilePathForSource('projectSettings')}`,
        value: saveDestination
      };
    case 'userSettings':
      return {
        label: 'User settings',
        description: `Saved in ~/.gizzi/settings.json`,
        value: saveDestination
      };
  }
}
type Props = {
  onAddRules: (rules: PermissionRule[], unreachable?: UnreachableRule[]) => void;
  onCancel: () => void;
  ruleValues: PermissionRuleValue[];
  ruleBehavior: PermissionBehavior;
  initialContext: ToolPermissionContext;
  setToolPermissionContext: (newContext: ToolPermissionContext) => void;
};
export function AddPermissionRules({
    onAddRules,
    onCancel,
    ruleValues,
    ruleBehavior,
    initialContext,
    setToolPermissionContext
}: Props) {
  const t1 = SOURCES.map(optionForPermissionSaveDestination);

  const allOptions = t1;
  const t2 = selectedValue => {
      if (selectedValue === "cancel") {
        onCancel();
        return;
      } else {
        if ((SOURCES as readonly string[]).includes(selectedValue)) {
          const destination = selectedValue as EditableSettingSource;
          const updatedContext = applyPermissionUpdate(initialContext, {
            type: "addRules",
            rules: ruleValues,
            behavior: ruleBehavior,
            destination
          });
          persistPermissionUpdate({
            type: "addRules",
            rules: ruleValues,
            behavior: ruleBehavior,
            destination
          });
          setToolPermissionContext(updatedContext);
          const rules = ruleValues.map(ruleValue => ({
            ruleValue,
            ruleBehavior,
            source: destination
          }));
          const sandboxAutoAllowEnabled = SandboxManager.isSandboxingEnabled() && SandboxManager.isAutoAllowBashIfSandboxedEnabled();
          const allUnreachable = detectUnreachableRules(updatedContext, {
            sandboxAutoAllowEnabled
          });
          const newUnreachable = allUnreachable.filter(u => ruleValues.some(rv => rv.toolName === u.rule.ruleValue.toolName && rv.ruleContent === u.rule.ruleValue.ruleContent));
          onAddRules(rules, newUnreachable.length > 0 ? newUnreachable : undefined);
        }
      }
    };

  const onSelect = t2;
  const t3 = plural(ruleValues.length, "rule");

  const title = `Add ${ruleBehavior} permission ${t3}`;
  const t4 = ruleValues.map(_temp);

  const t5 = <Box flexDirection="column" paddingX={2}>{t4}</Box>;

  const t6 = ruleValues.length === 1 ? "Where should this rule be saved?" : "Where should these rules be saved?";
  const t7 = <Text>{t6}</Text>;

  const t8 = <Select options={allOptions} onChange={onSelect} />;

  const t9 = <Box flexDirection="column" marginY={1}>{t7}{t8}</Box>;

  const t10 = <Dialog title={title} onCancel={onCancel} color="permission">{t5}{t9}</Dialog>;

  return t10;
}
function _temp(ruleValue_0) {
  return <Box flexDirection="column" key={permissionRuleValueToString(ruleValue_0)}><Text bold={true}>{permissionRuleValueToString(ruleValue_0)}</Text><PermissionRuleDescription ruleValue={ruleValue_0} /></Box>;
}
