import React from 'react';
import { Box, Link, Text } from '../../ink';
import type { Output } from './RequestSignatureTool';

/** Card for a signature request. Display only: nothing here signs or declines. */
export function renderToolUseMessage(input: { title?: string; id?: string; document_path?: string }): React.ReactNode {
  return input.title ?? (input.id ? `status of ${input.id}` : '');
}

function docName(path: string | undefined): string | undefined {
  return path ? path.split(/[\\/]/).pop() : undefined;
}

function when(iso: string | undefined): string {
  if (!iso) return '';
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

const SETTLED: Record<string, string> = {
  declined: 'Declined',
  cancelled: 'Cancelled',
  canceled: 'Cancelled',
  expired: 'Expired'
};

export function renderToolResultMessage(output: Output, _progress?: unknown, options?: { input?: { document_path?: string } }): React.ReactNode {
  const doc = docName(options?.input?.document_path);
  const title = output.doc_title ?? output.title.replace(/^Signature requested: /, '');
  const status = output.status.toLowerCase();
  const pending = status === 'pending' || status === 'open' || status === 'sent' || status === 'requested';
  const signed = status === 'signed';
  return <Box flexDirection="row" marginTop={1}>
      <Box minWidth={2} />
      <Box flexDirection="column">
        {pending ? <Text color="warning" bold>Awaiting your signature</Text> : signed ? <Text color="success" bold>Signed</Text> : <Text bold>{SETTLED[status] ?? output.status}</Text>}
        {title && !title.startsWith('Signature ') ? <Text>{title}</Text> : null}
        {doc ? <Text dimColor>{doc}</Text> : null}
        {signed ? <Text dimColor>
            {output.signer_name ? `Signed by ${output.signer_name}` : 'Signed'}
            {output.resolved_at ? ` · ${when(output.resolved_at)}` : ''}
          </Text> : null}
        {pending ? <>
            <Link url={output.sign_url}>{output.sign_url}</Link>
            <Text dimColor>Sign on Desktop or your phone. The terminal cannot sign.</Text>
          </> : null}
      </Box>
    </Box>;
}
