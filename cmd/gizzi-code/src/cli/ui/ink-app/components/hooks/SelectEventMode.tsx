/**
 * SelectEventMode is the entrypoint of the Hooks config menu, where the user
 * sees the list of available hook events.
 *
 * The /hooks menu is read-only: selecting an event lets you browse its
 * configured hooks but not modify them. To add or change hooks, users should
 * edit settings.json directly or ask Claude.
 */

import figures from 'figures';
import * as React from 'react';
import type { HookEvent } from './../../entrypoints/agentSdkTypes.ts';
import type { HookEventMetadata } from './../../utils/hooks/hooksConfigManager.ts';
import { Box, Link, Text } from '../../ink';
import { plural } from '../../utils/stringUtils';
import { Select } from '../CustomSelect/select';
import { Dialog } from '../design-system/Dialog';
type Props = {
  hookEventMetadata: Record<HookEvent, HookEventMetadata>;
  hooksByEvent: Partial<Record<HookEvent, number>>;
  totalHooksCount: number;
  restrictedByPolicy: boolean;
  onSelectEvent: (event: HookEvent) => void;
  onCancel: () => void;
};
export function SelectEventMode({
    hookEventMetadata,
    hooksByEvent,
    totalHooksCount,
    restrictedByPolicy,
    onSelectEvent,
    onCancel
}: Props) {
  const t1 = plural(totalHooksCount, "hook");

  const subtitle = `${totalHooksCount} ${t1} configured`;
  const t2 = restrictedByPolicy && <Box flexDirection="column"><Text color="suggestion">{figures.info} Hooks Restricted by Policy</Text><Text dimColor={true}>Only hooks from managed settings can run. User-defined hooks from ~/.claude/settings.json, .claude/settings.json, and .claude/settings.local.json are blocked.</Text></Box>;

  const t3 = <Box flexDirection="column"><Text dimColor={true}>{figures.info} This menu is read-only. To add or modify hooks, edit settings.json directly or ask Gizzi.{" "}<Link url="https://docs.allternit.com/cli/configuration">Learn more</Link></Text></Box>;

  const t4 = value => {
      onSelectEvent(value as HookEvent);
    };

  const t5 = Object.entries(hookEventMetadata);

  const t6 = t5.map(t7 => {
      const [name, metadata] = t7;
      const count = hooksByEvent[name as HookEvent] || 0;
      return {
        label: count > 0 ? <Text>{name} <Text color="suggestion">({count})</Text></Text> : name,
        value: name,
        description: metadata.summary
      };
    });

  const t7 = <Box flexDirection="column"><Select onChange={t4} onCancel={onCancel} options={t6} /></Box>;

  const t8 = <Box flexDirection="column" gap={1}>{t2}{t3}{t7}</Box>;

  const t9 = <Dialog title="Hooks" subtitle={subtitle} onCancel={onCancel}>{t8}</Dialog>;

  return t9;
}
