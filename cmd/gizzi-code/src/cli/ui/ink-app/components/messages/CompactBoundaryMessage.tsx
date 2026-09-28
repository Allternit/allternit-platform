import * as React from 'react';
import { useMainLoopModel } from '../../hooks/useMainLoopModel';
import { useShortcutDisplay } from '../../keybindings/useShortcutDisplay';
import type { CompactMetadata } from '../../types/message';
import { getContextWindowForModel } from '../../utils/context';
import { SessionRip } from './SessionRip';

/**
 * Where the conversation was compacted into a fresh context window — drawn
 * as the rip (P3.16), the same divider Desktop shows. The checkpoint (the
 * compact summary) is in the transcript view; the earlier conversation stays
 * in scrollback above the rip (terminal scrollback, or fullscreen's).
 */
export function CompactBoundaryMessage({ metadata, timestamp }: { metadata?: CompactMetadata; timestamp?: string }) {
  const historyShortcut = useShortcutDisplay("app:toggleTranscript", "Global", "ctrl+o");
  const model = useMainLoopModel();
  const preTokens = typeof metadata?.preTokens === 'number' ? metadata.preTokens : 0;
  const window = getContextWindowForModel(model);
  // A handoff followed in from another client (bots pane) carries its own
  // generation and reason; a local compaction is either /compact or auto.
  const handoff = metadata?.handoff as { generation?: number; reason?: string } | undefined;
  const reason = handoff ? handoff.reason : metadata?.trigger === 'manual' ? 'you ran /compact' : 'threshold';
  return <SessionRip startedAt={timestamp} generation={handoff?.generation} reason={reason} previousUse={preTokens > 0 && window > 0 ? preTokens / window : null} hint={handoff ? `${historyShortcut} for the checkpoint` : `earlier messages above · ${historyShortcut} for the checkpoint`} />;
}
