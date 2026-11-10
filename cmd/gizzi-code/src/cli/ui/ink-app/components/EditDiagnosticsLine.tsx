import * as React from 'react';
import { useSyncExternalStore } from 'react';
import { Text } from '../ink';
import { MessageResponse } from './MessageResponse';
import { formatEditDiagnostics, getEditDiagnostics, subscribeEditDiagnostics } from '../services/lsp/editDiagnostics';

/**
 * The language server's new errors for one edit, under the edit's summary:
 * "✗ 2 new errors · L3:7 Type 'string' is not assignable … (+1 more)".
 * Renders nothing while the server is still checking, when no server handles
 * the file, or when the edit left no new errors.
 */
export function EditDiagnosticsLine({ lspKey, ownRow }: { lspKey?: string; ownRow?: boolean }): React.ReactNode {
  const entry = useSyncExternalStore(subscribeEditDiagnostics, () => getEditDiagnostics(lspKey));
  const line = formatEditDiagnostics(entry);
  if (!line) return null;
  const text = <Text color="error" wrap="truncate-end">✗ {line}</Text>;
  return ownRow ? <MessageResponse>{text}</MessageResponse> : text;
}
