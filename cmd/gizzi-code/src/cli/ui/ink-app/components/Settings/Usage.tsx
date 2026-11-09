import * as React from 'react';
import { useCallback, useEffect, useState } from 'react';
import { accountUsageLines } from '../../commands/usage/usage';
import { Box, Text } from '../../ink';
import { useKeybinding } from '../../keybindings/useKeybinding';
import { getAllternitAccount } from '../../services/allternitAccount';
import { logError } from '../../utils/log';
import { ConfigurableShortcutHint } from '../ConfigurableShortcutHint';
import { Byline } from '../design-system/Byline';

/**
 * Settings › Usage: the Allternit plan (Free / Plus / Max / Ultra), credits
 * and Cloud balance, then each signed-in CLI brain's plan limits. Same lines
 * as `/usage`; no Anthropic plan is assumed.
 */
export function Usage(): React.ReactNode {
  const [lines, setLines] = useState<string[] | null>(null);
  const load = useCallback(async (fresh: boolean) => {
    setLines(null);
    try {
      if (fresh) await getAllternitAccount({ fresh: true });
      setLines(await accountUsageLines());
    } catch (err) {
      logError(err as Error);
      setLines(['Couldn\'t load usage.']);
    }
  }, []);
  useEffect(() => {
    void load(false);
  }, [load]);
  useKeybinding('settings:retry', () => {
    void load(true);
  }, {
    context: 'Settings',
    isActive: lines !== null
  });
  return <Box flexDirection="column" gap={1} width="100%">
      {lines === null ? <Text dimColor>Loading usage…</Text> : <Text>{lines.join('\n')}</Text>}
      <Text dimColor>
        <Byline>
          <ConfigurableShortcutHint action="settings:retry" context="Settings" fallback="r" description="refresh" />
          <ConfigurableShortcutHint action="confirm:no" context="Settings" fallback="Esc" description="cancel" />
        </Byline>
      </Text>
    </Box>;
}
