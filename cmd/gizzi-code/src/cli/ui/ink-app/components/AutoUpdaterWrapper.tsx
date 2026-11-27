import * as React from 'react';
import { Updater } from '@/shared/installation/updater';
import { Box, Text } from '../ink';

/**
 * The one update notice in the prompt footer. It only reads the signed
 * updater (src/shared/installation/updater.ts); the updater itself is started
 * once per process by the TUI entry (app.tsx) and does all checking,
 * downloading and swapping on its own.
 *
 * Shows a line only when the user has something to know or do:
 *   staged / scheduled  "Gizzi vX ready — applies when you exit" (dim)
 *   available           "Update available: run `<command>`" (brew/scoop/choco,
 *                       or auto-update off)
 *   required            the same facts in the warning color
 * Everything else (idle, checking, current, off, key-missing, paused,
 * downloading, managed by Allternit Desktop, error) shows nothing here;
 * `gizzi doctor` reports those.
 */
export function AutoUpdaterWrapper(): React.ReactNode {
  const [status, setStatus] = React.useState<Updater.Status>(() => Updater.getStatus());
  React.useEffect(() => {
    setStatus(Updater.getStatus());
    return Updater.onStatus(setStatus);
  }, []);

  const line = updateNoticeLine(status);
  if (!line) return null;
  return (
    <Box>
      <Text color={line.required ? 'warning' : undefined} dimColor={!line.required} wrap="truncate">
        {line.text}
      </Text>
    </Box>
  );
}

function v(version: string | undefined): string {
  if (!version) return '';
  return version.startsWith('v') ? version : `v${version}`;
}

/** Footer text for an updater status, or null when the footer stays quiet. Exported for tests. */
export function updateNoticeLine(status: Updater.Status): { text: string; required: boolean } | null {
  const latest = v(status.latest);
  const required = status.required === true;
  switch (status.state) {
    case 'staged':
    case 'scheduled':
      return required
        ? { text: `Gizzi ${latest} is required — it installs when you exit`, required }
        : { text: `Gizzi ${latest} ready — applies when you exit`, required };
    case 'available': {
      const command = status.command ?? 'gizzi update';
      return required
        ? { text: `Gizzi ${latest} is required (you have ${v(status.current)}): run \`${command}\``, required }
        : { text: `Update available: run \`${command}\``, required };
    }
    case 'downloading':
      return required ? { text: `Gizzi ${latest} is required — downloading it now`, required } : null;
    default:
      // idle, checking, current, off, key-missing, paused, managed, error
      return null;
  }
}
