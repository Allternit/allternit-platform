import * as React from 'react';
import { useEffect, useState } from 'react';
import { limitLabel, untilLabel } from '@/runtime/bots/session-pause.js';
import { Box, Text } from '../../ink';
import { useAppState } from '../../state/AppState';

/**
 * "⏸ Paused until 7:40 PM · Claude 5-hour limit · resumes on its own" (P3.17),
 * or, short of a pause, "◔ Approaching usage limit · …" past limits.warn_at,
 * the same copy as Desktop's composer bar and the pet HUD. Offers the model
 * with the most limit left only as an explicit `/resume-now`.
 */
export function PausedBar(): React.ReactNode {
  const pause = useAppState(s => s.replPause);
  const warning = useAppState(s => s.replLimitWarning);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!pause && !warning) return;
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, [pause, warning]);
  if (!pause || pause.until <= now) {
    // "◔ Approaching usage limit · 84% of Kimi 5-hour limit · Resets at 11:55 AM" — same copy as the web strip.
    if (!warning || (warning.resetAt !== undefined && warning.resetAt <= now)) return null;
    const label = limitLabel({ until: 0, limit: warning.limit, providerID: warning.providerID });
    return <Box paddingX={2}>
        <Text wrap="wrap">
          <Text color="warning">◔ Approaching usage limit</Text>
          <Text dimColor> · {Math.round(warning.usedRatio * 100)}% of {label}{warning.resetAt ? ` · Resets at ${untilLabel(warning.resetAt, now)}` : ''}</Text>
        </Text>
      </Box>;
  }
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
