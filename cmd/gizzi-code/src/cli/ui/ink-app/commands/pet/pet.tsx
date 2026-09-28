import * as React from 'react';
import { getCompanion } from '../../pet/companion.js';
import { PetHud } from '../../pet/PetHud.js';
import type { LocalJSXCommandOnDone } from '../../types/command.js';
import type { ProcessUserInputContext } from '../../utils/processUserInput/processUserInput.js';
import { getGlobalConfig, saveGlobalConfig } from '../../utils/config.js';

/**
 * /pet — the terminal pet wears the same Allternit bot as the Desktop pet
 * (Gizzi by default). `/pet` turns it on and opens the HUD; `pat`, `mute`
 * and `unmute` are quick actions.
 */
export async function call(onDone: LocalJSXCommandOnDone, context: ProcessUserInputContext, args: string): Promise<React.ReactNode> {
  const sub = String(args ?? '').trim().toLowerCase();
  const existing = getCompanion();

  if (sub === 'mute' || sub === 'unmute') {
    if (!existing) {
      onDone('Your pet is off. Run /pet to turn it on.', { display: 'system' });
      return null;
    }
    const muted = sub === 'mute';
    saveGlobalConfig(c => ({ ...c, companionMuted: muted }));
    if (muted) context.setAppState(prev => ({ ...prev, companionReaction: undefined }));
    onDone(muted ? `${existing.name} is napping. /pet unmute to wake it.` : `${existing.name} is back.`, { display: 'system' });
    return null;
  }

  if (sub === 'pat') {
    if (!existing) {
      onDone('Your pet is off. Run /pet to turn it on.', { display: 'system' });
      return null;
    }
    context.setAppState(prev => ({ ...prev, companionPetAt: Date.now() }));
    onDone(`You pat ${existing.name}.`, { display: 'system' });
    return null;
  }

  if (sub) {
    onDone('Usage: /pet [pat|mute|unmute]', { display: 'system' });
    return null;
  }

  const config = getGlobalConfig();
  if (!config.companion || config.companionMuted) {
    saveGlobalConfig(c => ({ ...c, companion: c.companion ?? { hatchedAt: Date.now() }, companionMuted: false }));
    if (!config.companion) context.setAppState(prev => ({ ...prev, companionPetAt: Date.now() }));
  }
  return <PetHud onDone={onDone} />;
}
