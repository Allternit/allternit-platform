import { mkdir, writeFile } from 'fs/promises';
import * as React from 'react';
import type { CommandResultDisplay } from '../../commands';
import { Dialog } from '../../components/design-system/Dialog';
import { MemoryFileSelector } from '../../components/memory/MemoryFileSelector';
import { getRelativeMemoryPath } from '../../components/memory/MemoryUpdateNotification';
import { Box, Link, Text } from '../../ink';
import type { LocalJSXCommandCall } from '../../types/command';
import { clearMemoryFileCaches, getMemoryFiles } from '../../utils/gizzimd';
import { getGizziConfigHomeDir } from '../../utils/envUtils';
import { getErrnoCode } from '../../utils/errors';
import { logError } from '../../utils/log';
import { editFileInEditor } from '../../utils/promptEditor';
import { getSessionId } from '../../bootstrap/state.js';
import { isAutoMemPath, isAutoMemoryEnabled, isMemoryDriveActive } from '../../memdir/paths.js';
import { MemoryDrive } from '../../../../../runtime/memory/drive/drive.js';
import { formatFileView, formatHistory, formatOverview, syncLine } from '../../../../../runtime/memory/drive/report.js';

type OnDone = (result?: string, options?: { display?: CommandResultDisplay }) => void;

const USAGE = [
  'Usage: /memory                 show your Memory Drive and edit memory files',
  '       /memory view <path>     show one drive file (e.g. /memory view preferences.md)',
  '       /memory log             recent drive history',
  '       /memory sync            commit edits in the checkout and sync now',
].join('\n');

function MemoryCommand({ onDone, driveSummary }: { onDone: OnDone; driveSummary?: string }): React.ReactNode {
  const handleSelectMemoryFile = async (memoryPath: string) => {
    try {
      if (memoryPath.includes(getGizziConfigHomeDir())) {
        await mkdir(getGizziConfigHomeDir(), { recursive: true });
      }
      const inDrive = isMemoryDriveActive() && isAutoMemPath(memoryPath);
      if (inDrive) {
        // The checkout is a git repo gizzi manages; create it properly, never a bare file.
        await (await MemoryDrive.open('personal')).ensure();
      } else {
        try {
          await writeFile(memoryPath, '', { encoding: 'utf8', flag: 'wx' });
        } catch (e: unknown) {
          if (getErrnoCode(e) !== 'EEXIST') throw e;
        }
      }
      await editFileInEditor(memoryPath);

      let saved = '';
      if (inDrive) {
        const result = await MemoryDrive.commitWorkingTree({
          sessionId: String(getSessionId() ?? ''),
          message: 'Edit memory by hand',
        });
        saved = result.error
          ? `\n\n${result.error}`
          : result.changed
            ? result.pending
              ? '\n\nSaved to your Memory Drive on this computer; it will sync when the server is reachable.'
              : '\n\nSaved and synced to your Memory Drive.'
            : '';
      }
      let editorSource = 'default';
      let editorValue = '';
      if (process.env.VISUAL) {
        editorSource = '$VISUAL';
        editorValue = process.env.VISUAL;
      } else if (process.env.EDITOR) {
        editorSource = '$EDITOR';
        editorValue = process.env.EDITOR;
      }
      const editorInfo = editorSource !== 'default' ? `Using ${editorSource}="${editorValue}".` : '';
      const editorHint = editorInfo
        ? `> ${editorInfo} To change editor, set $EDITOR or $VISUAL environment variable.`
        : `> To use a different editor, set the $EDITOR or $VISUAL environment variable.`;
      onDone(`Opened memory file at ${getRelativeMemoryPath(memoryPath)}${saved}\n\n${editorHint}`, { display: 'system' });
    } catch (error) {
      logError(error);
      onDone(`Error opening memory file: ${error}`);
    }
  };
  const handleCancel = () => {
    onDone('Cancelled memory editing', { display: 'system' });
  };
  return (
    <Dialog title="Memory" onCancel={handleCancel} color="remember">
      <Box flexDirection="column">
        {driveSummary ? (
          <Box flexDirection="column" marginBottom={1}>
            <Text>{driveSummary}</Text>
            <Text dimColor>/memory view &lt;path&gt; shows a file · /memory sync syncs now</Text>
          </Box>
        ) : null}
        <React.Suspense fallback={null}>
          <MemoryFileSelector onSelect={handleSelectMemoryFile} onCancel={handleCancel} />
        </React.Suspense>
        <Box marginTop={1}>
          <Text dimColor>
            Learn more: <Link url="https://docs.allternit.com/guides/memory-drive" />
          </Text>
        </Box>
      </Box>
    </Dialog>
  );
}

/** Text-only subcommands: view / log / sync. Returns undefined for the dialog. */
export async function memoryDriveSubcommand(args: string): Promise<string | undefined> {
  const [sub, ...rest] = args.trim().split(/\s+/).filter(Boolean);
  if (!sub) return undefined;
  if (sub === 'help') return USAGE;
  if (!isMemoryDriveActive()) {
    return 'The Memory Drive is off for this session (GIZZI_MEMORY_DRIVE=0 or a custom autoMemoryDirectory is set).';
  }
  if (sub === 'view' || sub === 'cat' || sub === 'show') {
    const rel = rest.join(' ').trim();
    if (!rel) return 'Pass a drive file, e.g. /memory view preferences.md';
    const target = rel.endsWith('.md') ? rel : `${rel}.md`;
    try {
      const drive = await MemoryDrive.open('personal');
      await drive.ensure();
      return formatFileView(target, await drive.readFile(target));
    } catch (error) {
      return `Could not read ${target}: ${error instanceof Error ? error.message : String(error)}`;
    }
  }
  if (sub === 'log' || sub === 'history') {
    const overview = await MemoryDrive.overview('personal', 25);
    return formatHistory(overview.history);
  }
  if (sub === 'sync') {
    const result = await MemoryDrive.commitWorkingTree({ sessionId: String(getSessionId() ?? ''), message: 'Save memory edits' });
    const status = await MemoryDrive.sync();
    const overview = await MemoryDrive.overview('personal', 0);
    return [result.error, syncLine(status, overview.signedIn)].filter(Boolean).join('\n');
  }
  return undefined;
}

export const call: LocalJSXCommandCall = async (onDone, _context, args) => {
  const text = await memoryDriveSubcommand(String(args ?? '')).catch((error) => `Memory Drive error: ${error instanceof Error ? error.message : String(error)}`);
  if (text !== undefined) {
    onDone(text, { display: 'system' });
    return null;
  }
  let driveSummary: string | undefined;
  if (isAutoMemoryEnabled() && isMemoryDriveActive()) {
    try {
      await Promise.race([MemoryDrive.prepare(), new Promise((r) => setTimeout(r, 3000))]);
      driveSummary = formatOverview(await MemoryDrive.overview('personal', 5));
    } catch (error) {
      driveSummary = `Memory Drive unavailable: ${error instanceof Error ? error.message : String(error)}`;
    }
  }
  clearMemoryFileCaches();
  await getMemoryFiles();
  return <MemoryCommand onDone={onDone} driveSummary={driveSummary} />;
};
