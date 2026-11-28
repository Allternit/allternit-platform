import { mkdir, writeFile } from 'fs/promises';
import * as React from 'react';
import type { CommandResultDisplay } from '../../commands';
import { Dialog } from '../../components/design-system/Dialog';
import { MemoryFileSelector } from '../../components/memory/MemoryFileSelector';
import { getRelativeMemoryPath } from '../../components/memory/MemoryUpdateNotification';
import { Box, Link, Text } from '../../ink';
import type { LocalJSXCommandCall } from '../../types/command';
import { memoryRoute, SLEEP_COMMAND_NAME } from './subcommand';
import { clearMemoryFileCaches, getMemoryFiles } from '../../utils/gizzimd';
import { getGizziConfigHomeDir } from '../../utils/envUtils';
import { getErrnoCode } from '../../utils/errors';
import { logError } from '../../utils/log';
import { editFileInEditor } from '../../utils/promptEditor';
import { getSessionId } from '../../bootstrap/state.js';
import { isAutoMemPath, isAutoMemoryEnabled, isMemoryDriveActive } from '../../memdir/paths.js';
import { MemoryDrive } from '../../../../../runtime/memory/drive/drive.js';
import { sleepApi, sleepStatusLine } from '../../../../../runtime/memory/sleep/client.js';
import { formatFileView, formatHistory, formatOverview, syncLine } from '../../../../../runtime/memory/drive/report.js';

type OnDone = (result?: string, options?: { display?: CommandResultDisplay }) => void;

const USAGE = [
  'Usage: /memory                 show your Memory Drive and edit memory files',
  '       /memory search <words>  search your Memory Drive',
  '       /memory add <note>      save a note to memory now',
  '       /memory view <path>     show one drive file (e.g. /memory view preferences.md)',
  '       /memory log             recent drive history',
  '       /memory sync            commit edits in the checkout and sync now',
  '       /memory sleep           open Sleep (last and next run, last report)',
  '       /memory sleep on|off    switch Sleep on or off',
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
            <Text dimColor>/memory search &lt;words&gt; · /memory add &lt;note&gt; · Sleep: /memory sleep</Text>
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

/** Sleep rows for the /memory view: status line, conflicts, the switch. */
async function sleepSummary(): Promise<string | undefined> {
  const out = await sleepApi.status();
  if (out.kind !== 'ok') return out.kind === 'no_sleep' ? undefined : out.message;
  const s = out.body;
  const lines = [sleepStatusLine(s)];
  if (s.open_disputes > 0) lines.push(`${s.open_disputes} memory conflicts need you. Run \`gizzi memory disputes\` in a terminal.`);
  lines.push(s.enabled ? '/memory sleep off turns Sleep off' : '/memory sleep on turns Sleep on');
  return lines.join('\n');
}

/** Text-only subcommands: view / log / sync. Returns undefined for the dialog. */
export async function memoryDriveSubcommand(args: string): Promise<string | undefined> {
  const [sub, ...rest] = args.trim().split(/\s+/).filter(Boolean);
  if (!sub) return undefined;
  if (sub === 'help') return USAGE;
  if (sub === 'sleep') {
    const arg = rest[0];
    if (arg === 'on' || arg === 'off') {
      const out = await sleepApi.setEnabled(arg === 'on');
      return out.kind === 'ok' ? `Sleep is ${arg}.` : out.message;
    }
    return (await sleepSummary()) ?? 'This runtime has no Sleep yet. Update Allternit.';
  }
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

export const call: LocalJSXCommandCall = async (onDone, context, args) => {
  // `/memory search`, `/memory add` (were /memory-search, /remember) and the Sleep entry.
  const route = memoryRoute(String(args ?? ''));
  if (route?.kind === 'search') return (await import('../memory-search/memory-search')).call(onDone, context, route.rest);
  if (route?.kind === 'add') {
    const result = await (await import('../remember/remember')).call(route.rest, context);
    onDone(result.type === 'text' ? result.value : undefined, { display: 'system' });
    return null;
  }
  if (route?.kind === 'sleep-view') {
    // The Sleep view is its own command (PR #3235); builds without it keep the text summary below.
    const sleepView = (context.options?.commands ?? []).find(c => c.name === SLEEP_COMMAND_NAME);
    if (sleepView?.type === 'local-jsx') return (await sleepView.load()).call(onDone, context, '');
  }
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
  const sleepText = await sleepSummary().catch(() => undefined);
  if (sleepText) driveSummary = driveSummary ? `${driveSummary}\n\n${sleepText}` : sleepText;
  clearMemoryFileCaches();
  await getMemoryFiles();
  return <MemoryCommand onDone={onDone} driveSummary={driveSummary} />;
};
