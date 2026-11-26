/**
 * SelectHookMode shows all hooks configured for a given event+matcher pair.
 *
 * The /hooks menu is read-only: this view no longer offers "add new hook"
 * and selecting a hook shows its read-only details instead of a delete
 * confirmation.
 */
import * as React from 'react';
import type { HookEvent } from './../../entrypoints/agentSdkTypes.ts';
import type { HookEventMetadata } from './../../utils/hooks/hooksConfigManager.ts';
import { Box, Text } from '../../ink';
import { getHookDisplayText, hookSourceHeaderDisplayString, type IndividualHookConfig } from '../../utils/hooks/hooksSettings';
import { Select } from '../CustomSelect/select';
import { Dialog } from '../design-system/Dialog';
import { EmptyLine } from '../EmptyLine'
type Props = {
  selectedEvent: HookEvent;
  selectedMatcher: string | null;
  hooksForSelectedMatcher: IndividualHookConfig[];
  hookEventMetadata: HookEventMetadata;
  onSelect: (hook: IndividualHookConfig) => void;
  onCancel: () => void;
};
export function SelectHookMode({
    selectedEvent,
    selectedMatcher,
    hooksForSelectedMatcher,
    hookEventMetadata,
    onSelect,
    onCancel
}: Props) {
  const title = hookEventMetadata.matcherMetadata !== undefined ? `${selectedEvent} - Matcher: ${selectedMatcher || "(all)"}` : selectedEvent;
  if (hooksForSelectedMatcher.length === 0) {
    const t1 = <Box flexDirection="column" gap={1}><EmptyLine>No hooks configured for this event.</EmptyLine><Text dimColor={true}>To add hooks, edit settings.json directly or ask Gizzi.</Text></Box>;

    const t2 = <Dialog title={title} subtitle={hookEventMetadata.description} onCancel={onCancel} inputGuide={_temp}>{t1}</Dialog>;

    return t2;
  }
  const t1 = hookEventMetadata.description;
  const t2 = hooksForSelectedMatcher.map(_temp2);

  const t3 = value => {
      const index_0 = parseInt(value, 10);
      const hook_0 = hooksForSelectedMatcher[index_0];
      if (hook_0) {
        onSelect(hook_0);
      }
    };

  const t4 = <Box flexDirection="column"><Select options={t2} onChange={t3} onCancel={onCancel} /></Box>;

  const t5 = <Dialog title={title} subtitle={t1} onCancel={onCancel}>{t4}</Dialog>;

  return t5;
}
function _temp2(hook, index) {
  return {
    label: `[${hook.config.type}] ${getHookDisplayText(hook.config)}`,
    value: index.toString(),
    description: hook.source === "pluginHook" && hook.pluginName ? `${hookSourceHeaderDisplayString(hook.source)} (${hook.pluginName})` : hookSourceHeaderDisplayString(hook.source)
  };
}
function _temp() {
  return <Text>Esc to go back</Text>;
}
