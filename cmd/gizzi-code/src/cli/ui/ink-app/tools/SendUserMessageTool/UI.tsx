import figures from 'figures';
import React from 'react';
import { Markdown } from '../../components/Markdown';
import { Box, Text } from '../../ink';
import { getDisplayPath } from '../../utils/file';
import { formatFileSize } from '../../utils/format';
import type { Output } from './SendUserMessageTool';

export function renderToolUseMessage(input: { notify?: string }): React.ReactNode {
  return input.notify && input.notify !== 'none' ? `notify: ${input.notify}` : '';
}

const PUSH_LABEL: Record<Output['push'], string | null> = {
  sent: 'Push requested',
  not_requested: null,
  rate_limited: 'Push not sent: too many recently',
  unpaired: 'Push not sent: this runtime is not linked to your cloud account',
  unavailable: 'Push not sent: not connected to your Allternit account',
};

export function renderToolResultMessage(output: Output): React.ReactNode {
  const push = output.notify !== 'none' ? PUSH_LABEL[output.push] : null;
  return <Box flexDirection="row" marginTop={1}>
      <Box minWidth={2} />
      <Box flexDirection="column">
        <Markdown>{output.text}</Markdown>
        {output.files.length > 0 ? <Box flexDirection="column" marginTop={1}>
            {output.files.map(file => <Box key={file.path} flexDirection="row">
                <Text dimColor>{figures.pointerSmall} [{file.display === 'render' ? 'view' : 'file'}] </Text>
                <Text>{getDisplayPath(file.path)}</Text>
                <Text dimColor>
                  {' '}({formatFileSize(file.size)}{file.id ? ', in Agent Output' : ''})
                  {file.caption ? ` ${file.caption}` : ''}
                </Text>
              </Box>)}
          </Box> : null}
        {push ? <Text dimColor>{push}</Text> : null}
      </Box>
    </Box>;
}
