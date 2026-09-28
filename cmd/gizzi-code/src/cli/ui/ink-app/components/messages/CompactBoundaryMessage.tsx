import * as React from 'react';
import { useMainLoopModel } from '../../hooks/useMainLoopModel';
import { useShortcutDisplay } from '../../keybindings/useShortcutDisplay';
import type { CompactMetadata } from '../../types/message';
import { getContextWindowForModel } from '../../utils/context';
import { SessionRip } from './SessionRip';

/**
 * Where the conversation was compacted into a fresh context window — drawn
 * as the rip (P3.16), the same divider Desktop shows. The checkpoint (the
 * compact summary) and the earlier conversation are in the transcript view.
 */
export function CompactBoundaryMessage({ metadata, timestamp }: { metadata?: CompactMetadata; timestamp?: string }) {
  const historyShortcut = useShortcutDisplay("app:toggleTranscript", "Global", "ctrl+o");
  const model = useMainLoopModel();
  const preTokens = typeof metadata?.preTokens === 'number' ? metadata.preTokens : 0;
  const window = getContextWindowForModel(model);
  const reason = metadata?.trigger === 'manual' ? 'you ran /compact' : 'threshold';
  return <SessionRip startedAt={timestamp} reason={reason} previousUse={preTokens > 0 && window > 0 ? preTokens / window : null} hint={`${historyShortcut} for the checkpoint and earlier conversation`} />;
}
