import * as React from 'react';
import { Box, Text } from '../../ink';
import { useShortcutDisplay } from '../../keybindings/useShortcutDisplay';

/**
 * Dim key hint under the prompt while a turn runs and the prompt has text:
 * enter steers now, tab queues for after the turn, ctrl+x enter stops the
 * turn and sends. Reads the live bindings so rebinding updates the hint.
 */
export function SteerHint(): React.ReactNode {
  const steer = useShortcutDisplay('chat:submit', 'Chat', 'enter');
  const queue = useShortcutDisplay('chat:queueAfter', 'Chat', 'tab');
  const stop = useShortcutDisplay('chat:stopAndSend', 'Chat', 'ctrl+x enter');
  return (
    <Box paddingLeft={2}>
      <Text dimColor wrap="truncate-end">
        <Text bold>{steer}</Text> steer now · <Text bold>{queue}</Text> queue for after · <Text bold>{stop}</Text> stop + send
      </Text>
    </Box>
  );
}
