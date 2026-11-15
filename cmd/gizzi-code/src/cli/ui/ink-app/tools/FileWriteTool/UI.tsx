import type { ToolResultBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/index.mjs';
import type { StructuredPatchHunk } from 'diff';
import { isAbsolute, relative, resolve } from 'path';
import * as React from 'react';
import { Suspense, use, useState } from 'react';
import { EditDiagnosticsLine } from '../../components/EditDiagnosticsLine';
import { MessageResponse } from './../../components/MessageResponse.tsx';
import { extractTag } from './../../utils/extractTag.js';
import { CtrlOToExpand } from '../../components/CtrlOToExpand';
import { FallbackToolUseErrorMessage } from '../../components/FallbackToolUseErrorMessage';
import { FileEditToolUpdatedMessage } from '../../components/FileEditToolUpdatedMessage';
import { FileEditToolUseRejectedMessage } from '../../components/FileEditToolUseRejectedMessage';
import { FilePathLink } from '../../components/FilePathLink';
import { HighlightedCode } from '../../components/HighlightedCode';
import { useTerminalSize } from '../../hooks/useTerminalSize';
import { Box, Text } from '../../ink';
import type { ToolProgressData } from '../../Tool';
import type { ProgressMessage } from '../../types/message';
import { getCwd } from '../../utils/cwd';
import { getPatchForDisplay } from '../../utils/diff';
import { getDisplayPath } from '../../utils/file';
import { logError } from '../../utils/log';
import { getPlansDirectory } from '../../utils/plans';
import { openForScan, readCapped } from '../../utils/readEditContext';
import type { Output } from './FileWriteTool';
const MAX_LINES_TO_RENDER = 10;
// Model output uses \n regardless of platform, so always split on \n.
// os.EOL is \r\n on Windows, which would give numLines=1 for all files.
const EOL = '\n';

/**
 * Count visible lines in file content. A trailing newline is treated as a
 * line terminator (not a new empty line), matching editor line numbering.
 */
export function countLines(content: string): number {
  const parts = content.split(EOL);
  return content.endsWith(EOL) ? parts.length - 1 : parts.length;
}
function FileWriteToolCreatedMessage(t0) {
  const {
    filePath,
    content,
    verbose
  } = t0;
  const {
    columns
  } = useTerminalSize();
  const contentWithFallback = content || "(No content)";
  const numLines = countLines(content);
  const plusLines = numLines - MAX_LINES_TO_RENDER;
  const t1 = <Text bold={true}>{numLines}</Text>;

  const t2 = verbose ? filePath : relative(getCwd(), filePath);

  const t3 = <Text bold={true}><FilePathLink filePath={filePath}>{t2}</FilePathLink></Text>;

  const t4 = <Text>Wrote {t1} lines to{" "}{t3}</Text>;

  const t5 = verbose ? contentWithFallback : contentWithFallback.split("\n").slice(0, MAX_LINES_TO_RENDER).join("\n");

  const t6 = columns - 12;
  const t7 = <Box flexDirection="column"><HighlightedCode code={t5} filePath={filePath} width={t6} /></Box>;

  const t8 = !verbose && plusLines > 0 && <Text dimColor={true}>… +{plusLines} {plusLines === 1 ? "line" : "lines"}{" "}{numLines > 0 && <CtrlOToExpand />}</Text>;

  const t9 = <MessageResponse><Box flexDirection="column">{t4}{t7}{t8}</Box></MessageResponse>;

  return t9;
}
export function userFacingName(input: Partial<{
  file_path: string;
  content: string;
}> | undefined): string {
  if (input?.file_path?.startsWith(getPlansDirectory())) {
    return 'Updated plan';
  }
  return 'Write';
}

/** Gates fullscreen click-to-expand. Only `create` truncates (to
 *  MAX_LINES_TO_RENDER); `update` renders the full diff regardless of verbose.
 *  Called per visible message on hover/scroll, so early-exit after finding the
 *  (MAX+1)th line instead of splitting the whole (possibly huge) content. */
export function isResultTruncated({
  type,
  content
}: Output): boolean {
  if (type !== 'create') return false;
  let pos = 0;
  for (let i = 0; i < MAX_LINES_TO_RENDER; i++) {
    pos = content.indexOf(EOL, pos);
    if (pos === -1) return false;
    pos++;
  }
  // countLines treats a trailing EOL as a terminator, not a new line
  return pos < content.length;
}
export function getToolUseSummary(input: Partial<{
  file_path: string;
  content: string;
}> | undefined): string | null {
  if (!input?.file_path) {
    return null;
  }
  return getDisplayPath(input.file_path);
}
export function renderToolUseMessage(input: Partial<{
  file_path: string;
  content: string;
}>, {
  verbose
}: {
  verbose: boolean;
}): React.ReactNode {
  if (!input.file_path) {
    return null;
  }
  // For plan files, path is already in userFacingName
  if (input.file_path.startsWith(getPlansDirectory())) {
    return '';
  }
  return <FilePathLink filePath={input.file_path}>
      {verbose ? input.file_path : getDisplayPath(input.file_path)}
    </FilePathLink>;
}
export function renderToolUseRejectedMessage({
  file_path,
  content
}: {
  file_path: string;
  content: string;
}, {
  style,
  verbose
}: {
  style?: 'condensed';
  verbose: boolean;
}): React.ReactNode {
  return <WriteRejectionDiff filePath={file_path} content={content} style={style} verbose={verbose} />;
}
type RejectionDiffData = {
  type: 'create';
} | {
  type: 'update';
  patch: StructuredPatchHunk[];
  oldContent: string;
} | {
  type: 'error';
};
function WriteRejectionDiff(t0) {
  const {
    filePath,
    content,
    style,
    verbose
  } = t0;
  const t1 = () => loadRejectionDiff(filePath, content);

  const [dataPromise] = useState(t1);
  const t2 = content.split("\n")[0] ?? null;

  const firstLine = t2;
  const t3 = <FileEditToolUseRejectedMessage file_path={filePath} operation="write" content={content} firstLine={firstLine} verbose={verbose} />;

  const createFallback = t3;
  const t4 = <WriteRejectionBody promise={dataPromise} filePath={filePath} firstLine={firstLine} createFallback={createFallback} style={style} verbose={verbose} />;

  const t5 = <Suspense fallback={createFallback}>{t4}</Suspense>;

  return t5;
}
function WriteRejectionBody(t0: {
  promise: Promise<RejectionDiffData>;
  filePath: string;
  firstLine: string | null;
  createFallback: React.ReactElement;
  style?: 'condensed';
  verbose: boolean;
}) {
  const {
    promise,
    filePath,
    firstLine,
    createFallback,
    style,
    verbose
  } = t0;
  const data = use(promise);
  if (data.type === "create") {
    return createFallback;
  }
  if (data.type === "error") {
    const t1 = <MessageResponse><Text>(No changes)</Text></MessageResponse>;

    return t1;
  }
  const t1 = <FileEditToolUseRejectedMessage file_path={filePath} operation="update" patch={data.patch} firstLine={firstLine} fileContent={data.oldContent} style={style} verbose={verbose} />;

  return t1;
}
async function loadRejectionDiff(filePath: string, content: string): Promise<RejectionDiffData> {
  try {
    const fullFilePath = isAbsolute(filePath) ? filePath : resolve(getCwd(), filePath);
    const handle = await openForScan(fullFilePath);
    if (handle === null) return {
      type: 'create'
    };
    let oldContent: string | null;
    try {
      oldContent = await readCapped(handle);
    } finally {
      await handle.close();
    }
    // File exceeds MAX_SCAN_BYTES — fall back to the create view rather than
    // OOMing on a diff of a multi-GB file.
    if (oldContent === null) return {
      type: 'create'
    };
    const patch = getPatchForDisplay({
      filePath,
      fileContents: oldContent,
      edits: [{
        old_string: oldContent,
        new_string: content,
        replace_all: false
      }]
    });
    return {
      type: 'update',
      patch,
      oldContent
    };
  } catch (e) {
    // User may have manually applied the change while the diff was shown.
    logError(e as Error);
    return {
      type: 'error'
    };
  }
}
export function renderToolUseErrorMessage(result: ToolResultBlockParam['content'], {
  verbose
}: {
  verbose: boolean;
}): React.ReactNode {
  if (!verbose && typeof result === 'string' && extractTag(result, 'tool_use_error')) {
    return <MessageResponse>
        <Text color="error">Error writing file</Text>
      </MessageResponse>;
  }
  return <FallbackToolUseErrorMessage result={result} verbose={verbose} />;
}
export function renderToolResultMessage({
  filePath,
  content,
  structuredPatch,
  type,
  originalFile,
  lspKey
}: Output, _progressMessagesForMessage: ProgressMessage<ToolProgressData>[], {
  style,
  verbose
}: {
  style?: 'condensed';
  verbose: boolean;
}): React.ReactNode {
  switch (type) {
    case 'create':
      {
        const isPlanFile = filePath.startsWith(getPlansDirectory());

        // Plan files: invert condensed behavior
        // - Regular mode: just show hint (user can type /plan to see full content)
        // - Condensed mode (subagent view): show full content
        if (isPlanFile && !verbose) {
          if (style !== 'condensed') {
            return <MessageResponse>
              <Text dimColor>/plan to preview</Text>
            </MessageResponse>;
          }
        } else if (style === 'condensed' && !verbose) {
          const numLines = countLines(content);
          return <Text>
            Wrote <Text bold>{numLines}</Text> lines to{' '}
            <Text bold><FilePathLink filePath={filePath}>{relative(getCwd(), filePath)}</FilePathLink></Text>
          </Text>;
        }
        return <>
          <FileWriteToolCreatedMessage filePath={filePath} content={content} verbose={verbose} />
          {lspKey ? <EditDiagnosticsLine lspKey={lspKey} ownRow /> : null}
        </>;
      }
    case 'update':
      {
        const isPlanFile = filePath.startsWith(getPlansDirectory());
        return <FileEditToolUpdatedMessage filePath={filePath} structuredPatch={structuredPatch} firstLine={content.split('\n')[0] ?? null} fileContent={originalFile ?? undefined} style={style} verbose={verbose} previewHint={isPlanFile ? '/plan to preview' : undefined} lspKey={lspKey} />;
      }
  }
}
