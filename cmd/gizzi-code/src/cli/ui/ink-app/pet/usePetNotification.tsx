import { feature } from 'bun:bundle';
import React, { useEffect } from 'react';
import { useNotifications } from '../../../../context/notifications';
import { Text } from '../ink';
import { getGlobalConfig } from '../utils/config';
import { getRainbowColor } from '../utils/thinking';

// Local date, not UTC — 24h rolling wave across timezones. Sustained Twitter
// buzz instead of a single UTC-midnight spike, gentler on soul-gen load.
// Teaser window: April 1-7, 2026 only. Command stays live forever after.
export function isPetTeaserWindow(): boolean {
  if (("external" as string) === 'ant') return true;
  const d = new Date();
  return d.getFullYear() === 2026 && d.getMonth() === 3 && d.getDate() <= 7;
}
export function isPetLive(): boolean {
  if (("external" as string) === 'ant') return true;
  const d = new Date();
  return d.getFullYear() > 2026 || d.getFullYear() === 2026 && d.getMonth() >= 3;
}
function RainbowText(t0) {
  const {
    text
  } = t0;
  const t1 = <>{[...text].map(_temp)}</>;

  return t1;
}

// Rainbow /pet teaser shown on startup when no companion hatched yet.
// Idle presence and reactions are handled by CompanionSprite directly.
function _temp(ch, i) {
  return <Text key={i} color={getRainbowColor(i)}>{ch}</Text>;
}
export function usePetNotification() {
  const {
    addNotification,
    removeNotification
  } = useNotifications();
  const t0 = () => {
      if (!feature("PET")) {
        return;
      }
      const config = getGlobalConfig();
      if (config.companion || !isPetTeaserWindow()) {
        return;
      }
      addNotification({
        key: "pet-teaser",
        jsx: <RainbowText text="/pet" />,
        priority: "immediate",
        timeoutMs: 15000
      });
      return () => removeNotification("pet-teaser");
    };
  const t1 = [addNotification, removeNotification];

  useEffect(t0, t1);
}
export function findPetTriggerPositions(text: string): Array<{
  start: number;
  end: number;
}> {
  if (!feature('PET')) return [];
  const triggers: Array<{
    start: number;
    end: number;
  }> = [];
  const re = /\/pet\b/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    triggers.push({
      start: m.index,
      end: m.index + m[0].length
    });
  }
  return triggers;
}
