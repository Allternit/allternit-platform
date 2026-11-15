import { getGlobalConfig, saveGlobalConfig } from '../utils/config'
import { petOfferSettingsDefaults } from './petOffers.gen'
import { evaluatePetFailure } from './petOffers';
import { reportDesktopPetFailure, sayOnDesktopPet } from './desktopPetHook';

/** `/pet offers on|off`: kept in Gizzi's global config beside companionMuted. */
export function petOffersEnabled(): boolean {
  return getGlobalConfig().petOffersEnabled ?? petOfferSettingsDefaults().enabled
}

export function setPetOffersEnabled(enabled: boolean): void {
  saveGlobalConfig(c => ({ ...c, petOffersEnabled: enabled }))
}

/**
 * A turn error or failed command: tell the Desktop pet `failed`, record it for
 * /fix, and show the offer in the terminal pet's bubble and on Desktop when
 * the gate allows. The text stays in memory; nothing here logs or sends it.
 */
export function handlePetFailure(args: {
  cwd: string
  text: string
  needMatch: boolean
  say: (bubble: string) => void
}): void {
  const r = evaluatePetFailure({ cwd: args.cwd, text: args.text, needMatch: args.needMatch, enabled: petOffersEnabled() })
  if (!r) return
  reportDesktopPetFailure()
  if (r.offer) {
    args.say(r.offer)
    sayOnDesktopPet(r.offer)
  }
}
