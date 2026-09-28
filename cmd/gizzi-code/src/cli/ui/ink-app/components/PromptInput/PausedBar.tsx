import * as React from 'react';
import { useEffect, useState } from 'react';
import { limitLabel, untilLabel } from '@/runtime/bots/session-pause.js';
import { Box, Text } from '../../ink';
import { useAppState } from '../../state/AppState';

/**
 * "⏸ Paused until 7:40 PM · Claude 5-hour limit · resumes on its own" (P3.17),
 * the same copy as Desktop's composer bar and the pet HUD. Offers the model
 * with the most limit left only as an explicit `/resume-now`.
 */
export function PausedBar(): React.ReactNode {
  const pause = useAppState(s => s.replPause);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!pause) return;
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, [pause]);
  if (!pause || pause.until <= now) return null;
  const label = limitLabel({ until: pause.until, limit: pause.limit, providerID: pause.providerID });
  const s = pause.suggest;
  const alt = s ? ` · /resume-now to continue on ${s.label}${s.headroom !== undefined ? ` (${Math.round(s.headroom * 100)}% left)` : ''}` : '';
  return <Box paddingX={2}>
      <Text wrap="wrap">
        <Text color="warning">⏸ Paused until {untilLabel(pause.until, now)}</Text>
        <Text dimColor> · {label} · {pause.reason === 'limit_hit' ? 'continues on its own' : 'resumes on its own'}{alt}</Text>
      </Text>
    </Box>;
}
