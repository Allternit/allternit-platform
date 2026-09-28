import { getMainLoopModel } from '../utils/model/model'
import type { PetBot } from './petBots'
import { askCompanionModel, createUserMessage } from './soul'

export type ChatLine = { role: 'user' | 'assistant'; content: string }

/**
 * Offline incognito chat: when Allternit can't be reached, the pet still
 * answers on gizzi's own main model, in the bot's voice. Nothing is saved;
 * the conversation lives only in the HUD. No tools.
 */
export async function askPetLocally(history: ChatLine[], bot: PetBot, signal: AbortSignal): Promise<string | null> {
  const transcript = history
    .slice(-12)
    .map(line => `${line.role === 'user' ? 'User' : bot.name}: ${line.content}`)
    .join('\n\n')
  const system = `You are ${bot.name}${bot.description ? `, ${bot.description}` : ''}. The user is asking you a quick side question in their terminal. Answer directly and briefly, in plain text. You cannot run tools or read files in this chat.`
  return askCompanionModel(
    [createUserMessage({ content: `${transcript}\n\n${bot.name}:` })],
    system,
    signal,
    'pet_incognito',
    getMainLoopModel(),
  )
}
