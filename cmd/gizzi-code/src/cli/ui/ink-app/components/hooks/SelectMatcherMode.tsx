/**
 * SelectMatcherMode shows the configured matchers for a selected hook event.
 *
 * The /hooks menu is read-only: this view no longer offers "add new matcher"
 * and simply lets the user drill into each matcher to see its hooks.
 */
import * as React from 'react';
import type { HookEvent } from './../../entrypoints/agentSdkTypes.ts';
import { Box, Text } from '../../ink';
import { type HookSource, hookSourceInlineDisplayString, type IndividualHookConfig } from '../../utils/hooks/hooksSettings';
import { plural } from '../../utils/stringUtils';
import { Select } from '../CustomSelect/select';
import { Dialog } from '../design-system/Dialog';
import { EmptyLine } from '../EmptyLine'
type MatcherWithSource = {
  matcher: string;
  sources: HookSource[];
  hookCount: number;
};
type Props = {
  selectedEvent: HookEvent;
  matchersForSelectedEvent: string[];
  hooksByEventAndMatcher: Record<HookEvent, Record<string, IndividualHookConfig[]>>;
  eventDescription: string;
  onSelect: (matcher: string) => void;
  onCancel: () => void;
};
export function SelectMatcherMode({
    selectedEvent,
    matchersForSelectedEvent,
    hooksByEventAndMatcher,
    eventDescription,
    onSelect,
    onCancel
}: Props) {
  const t2 = matcher => {
        const hooks = hooksByEventAndMatcher[selectedEvent]?.[matcher] || [];
        const sources = Array.from(new Set(hooks.map(_temp)));
        return {
          matcher,
          sources,
          hookCount: hooks.length
        };
      };

  const t1 = matchersForSelectedEvent.map(t2);

  const matchersWithSources = t1;
  if (matchersForSelectedEvent.length === 0) {
    const t2 = `${selectedEvent} - Matchers`;
    const t3 = <Box flexDirection="column" gap={1}><EmptyLine>No hooks configured for this event.</EmptyLine><Text dimColor={true}>To add hooks, edit settings.json directly or ask Gizzi.</Text></Box>;

    const t4 = <Dialog title={t2} subtitle={eventDescription} onCancel={onCancel} inputGuide={_temp2}>{t3}</Dialog>;

    return t4;
  }
  const t2_2 = `${selectedEvent} - Matchers`;
  const t3 = matchersWithSources.map(_temp3);

  const t4 = value => {
      onSelect(value);
    };

  const t5 = <Box flexDirection="column"><Select options={t3} onChange={t4} onCancel={onCancel} /></Box>;

  const t6 = <Dialog title={t2_2} subtitle={eventDescription} onCancel={onCancel}>{t5}</Dialog>;

  return t6;
}
function _temp3(item) {
  const sourceText = item.sources.map(hookSourceInlineDisplayString).join(", ");
  const matcherLabel = item.matcher || "(all)";
  return {
    label: `[${sourceText}] ${matcherLabel}`,
    value: item.matcher,
    description: `${item.hookCount} ${plural(item.hookCount, "hook")}`
  };
}
function _temp2() {
  return <Text>Esc to go back</Text>;
}
function _temp(h) {
  return h.source;
}
