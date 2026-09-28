import { getGlobalConfig } from '../utils/config'
import { getCurrentPetBot, type PetBot } from './petBots'

/** The terminal pet: whichever bot the Desktop pet wears (Gizzi by default). */
export type Companion = {
  name: string
  personality: string
  bot: PetBot
}

// Called from hot paths (500ms sprite tick, per-keystroke PromptInput,
// per-turn observer); getCurrentPetBot is an in-memory lookup.
export function getCompanion(): Companion | undefined {
  if (!getGlobalConfig().companion) return undefined
  const bot = getCurrentPetBot()
  return {
    name: bot.name,
    personality: bot.description || `${bot.name}, one of the user's Allternit bots.`,
    bot,
  }
}
