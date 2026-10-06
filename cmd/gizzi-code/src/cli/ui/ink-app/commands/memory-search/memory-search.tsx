import * as React from 'react';
import { useEffect, useState } from 'react';
import type { CommandResultDisplay } from '../../commands';
import { Dialog } from '../../components/design-system/Dialog';
import { Box, Text } from '../../ink';
import type { LocalJSXCommandCall } from '../../types/command';
import { logError } from '../../utils/log.js';
import { isMemoryDriveActive } from '../../memdir/paths.js';
import { MemoryDrive } from '../../../../../runtime/memory/drive/drive.js';
import { formatSearch } from '../../../../../runtime/memory/drive/report.js';

/**
 * /memory-search <words> — search every Memory Drive file (personal first,
 * then mounted drives). Matches show drive:path:line, the fact, its date,
 * source and id.
 */
function MemorySearchCommand({
  query,
  onDone,
}: {
  query: string;
  onDone: (result?: string, options?: { display?: CommandResultDisplay }) => void;
}): React.ReactNode {
  const [text, setText] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (!isMemoryDriveActive()) {
      setText('The Memory Drive is off for this session (GIZZI_MEMORY_DRIVE=0 or a custom autoMemoryDirectory is set).');
      return;
    }
    MemoryDrive.search(query, 40)
      .then((hits) => !cancelled && setText(formatSearch(query, hits)))
      .catch((err) => {
        logError(err);
        if (!cancelled) setText(`Memory search failed: ${err instanceof Error ? err.message : String(err)}`);
      });
    return () => {
      cancelled = true;
    };
  }, [query]);

  return (
    <Dialog title="Search memory" onCancel={() => onDone(text ?? 'Cancelled', { display: 'system' })} color="remember">
      <Box flexDirection="column" gap={1}>
        {!query && <Text dimColor>Tip: /memory-search &lt;words&gt; — every word must appear on the line.</Text>}
        {text === null ? <Text dimColor>Searching your Memory Drive…</Text> : <Text>{text}</Text>}
      </Box>
    </Dialog>
  );
}

export const call: LocalJSXCommandCall = async (onDone, _context, args) => {
  return <MemorySearchCommand query={String(args ?? '').trim()} onDone={onDone} />;
};
