// @ts-nocheck
import * as React from 'react';
import { Box, Text } from '../../ink';
import { PermissionRuleList } from '../../components/permissions/rules/PermissionRuleList';
import type { LocalJSXCommandCall } from '../../types/command';
import { createPermissionRetryMessage } from '../../utils/messages';
import { useLoaded } from '../work/WorkPane';
import { accountToolBlockLabel, listAccountToolBlocks } from '@/cli/work/api';

/**
 * "From your account": the tools the account blocks (Settings > Connections >
 * Tool permissions), read-only. They are changed in the app, never here; the
 * runtime refuses them in every session the account runs.
 */
export function AccountToolBlocks(): React.ReactNode {
  const { data, error } = useLoaded(listAccountToolBlocks);
  return (
    <Box flexDirection="column" marginTop={1} paddingLeft={1}>
      <Text bold>From your account</Text>
      <Text dimColor>Blocked in your tool permissions. Change them in Allternit: Settings, Connections, Tool permissions.</Text>
      {error ? (
        <Text dimColor>{error}</Text>
      ) : data === undefined ? (
        <Text dimColor>Loading…</Text>
      ) : data.length === 0 ? (
        <Text dimColor>No tools are blocked on your account.</Text>
      ) : (
        data.map(block => (
          <Text key={`${block.ownerId}\u0000${block.toolId}`}>
            <Text color="error">Blocked</Text> {accountToolBlockLabel(block)}
          </Text>
        ))
      )}
    </Box>
  );
}

export const call: LocalJSXCommandCall = async (onDone, context) => {
  return (
    <Box flexDirection="column">
      <PermissionRuleList
        onExit={onDone}
        onRetryDenials={commands => {
          context.setMessages(prev => [...prev, createPermissionRetryMessage(commands)]);
        }}
      />
      <AccountToolBlocks />
    </Box>
  );
};
