import { getCompanion, roll, companionUserId } from '../../buddy/companion.js'
import { askCompanionModel, createUserMessage } from '../../buddy/soul.js'
import { RARITY_STARS, type CompanionSoul, type Species } from '../../buddy/types.js'
import type { LocalCommandCall } from '../../types/command.js'
import { getGlobalConfig, saveGlobalConfig } from '../../utils/config.js'

// Used when the model is unavailable, so hatching always works offline.
const FALLBACK_NAMES = ['Pip', 'Mote', 'Bix', 'Quill', 'Nib', 'Sprocket', 'Tuft', 'Wren', 'Fennel', 'Rook']
const FALLBACK_TRAITS = ['curious', 'unbothered', 'dramatic', 'deadpan', 'cheerful', 'suspicious', 'sleepy', 'meticulous']

function fallbackSoul(seed: number, species: Species): CompanionSoul {
  const name = FALLBACK_NAMES[seed % FALLBACK_NAMES.length]!
  const trait = FALLBACK_TRAITS[Math.floor(seed / 7) % FALLBACK_TRAITS.length]!
  return { name, personality: `A ${trait} little ${species} who has opinions about your code.` }
}

async function hatchSoul(): Promise<CompanionSoul> {
  const { bones, inspirationSeed } = roll(companionUserId())
  const statLine = Object.entries(bones.stats)
    .map(([k, v]) => `${k} ${v}`)
    .join(', ')
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 15_000)
  try {
    const reply = await askCompanionModel(
      [
        createUserMessage({
          content: `Invent a terminal companion. Species: ${bones.species}. Rarity: ${bones.rarity}. Stats: ${statLine}. Inspiration seed: ${inspirationSeed}.
Return ONLY JSON: {"name": "<one short name, max 12 chars>", "personality": "<one sentence, max 120 chars, shaped by the peak and dump stats>"}`,
        }),
      ],
      'You name and characterize small companion creatures. Output JSON only.',
      controller.signal,
      'companion_hatch',
    )
    const json = reply?.match(/\{[\s\S]*\}/)?.[0]
    if (json) {
      const parsed = JSON.parse(json) as Partial<CompanionSoul>
      const name = String(parsed.name ?? '').trim().slice(0, 12)
      const personality = String(parsed.personality ?? '').trim().slice(0, 160)
      if (name && personality) return { name, personality }
    }
  } catch {
    // fall through to the offline soul
  } finally {
    clearTimeout(timer)
  }
  return fallbackSoul(inspirationSeed, bones.species)
}

function card(): string {
  const c = getCompanion()!
  const stats = Object.entries(c.stats)
    .map(([k, v]) => `${k.toLowerCase()} ${v}`)
    .join(' · ')
  return `${c.name} the ${c.shiny ? 'shiny ' : ''}${c.species} ${RARITY_STARS[c.rarity]}\n${c.personality}\n${stats}\n\n/buddy pet · /buddy mute · /buddy unmute · say ${c.name}'s name to talk to it`
}

export const call: LocalCommandCall = async (args, context) => {
  const sub = String(args ?? '').trim().toLowerCase()
  const existing = getCompanion()

  if (sub === 'mute' || sub === 'unmute') {
    if (!existing) return { type: 'text', value: 'No companion yet. Run /buddy to hatch one.' }
    const muted = sub === 'mute'
    saveGlobalConfig(c => ({ ...c, companionMuted: muted }))
    if (muted) context.setAppState(prev => ({ ...prev, companionReaction: undefined }))
    return { type: 'text', value: muted ? `${existing.name} is napping. /buddy unmute to wake it.` : `${existing.name} is back.` }
  }

  if (sub === 'pet') {
    if (!existing) return { type: 'text', value: 'No companion yet. Run /buddy to hatch one.' }
    context.setAppState(prev => ({ ...prev, companionPetAt: Date.now() }))
    return { type: 'text', value: `You pet ${existing.name}.` }
  }

  if (sub) return { type: 'text', value: 'Usage: /buddy [pet|mute|unmute]' }

  if (existing) {
    if (getGlobalConfig().companionMuted) {
      saveGlobalConfig(c => ({ ...c, companionMuted: false }))
    }
    return { type: 'text', value: card() }
  }

  const soul = await hatchSoul()
  saveGlobalConfig(c => ({ ...c, companion: { ...soul, hatchedAt: Date.now() }, companionMuted: false }))
  context.setAppState(prev => ({ ...prev, companionPetAt: Date.now() }))
  return { type: 'text', value: `An egg hatched!\n\n${card()}` }
}
