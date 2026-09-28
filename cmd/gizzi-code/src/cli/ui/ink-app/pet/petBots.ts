import { useSyncExternalStore } from 'react'
import { PlatformApiError, PlatformSignedOutError, platformRequest } from '@/runtime/bots/platform-api.js'
import type { ModelRef } from '@/runtime/bots/platform-threads.js'
import { readDesktopPetAgentId, watchDesktopPetAgentId, writeDesktopPetAgentId } from './desktopPet'

/**
 * The bots the pet can wear. The pet is one of the user's Allternit bots (the
 * same one the Desktop pet wears); Gizzi is the default and the only one
 * available offline. Avatars mirror allternit-ai BotAvatar: a stored
 * geometric or pet avatar is drawn as such; an image avatar shows the image
 * in terminals that can draw images (iTerm2, WezTerm, Ghostty, Kitty) and
 * Gizzi elsewhere; everything else (mascot templates, colors) is Gizzi.
 */
export type PetAvatar =
  | { kind: 'gizzi' }
  | {
      kind: 'geometric'
      shape: 'circle' | 'rounded' | 'square' | 'hex' | 'diamond'
      primary: string
      secondary: string
      eyes: 'round' | 'wide' | 'narrow' | 'focused' | 'curious'
    }
  | {
      kind: 'pet'
      species: 'cat' | 'dog' | 'rabbit' | 'fox' | 'owl' | 'robot'
      primary: string
      secondary: string
      accessory: 'none' | 'glasses' | 'bow' | 'headset'
    }
  | { kind: 'image'; url: string }

export interface PetBot {
  id: string
  name: string
  description: string
  /** botProfile.accentColor; undefined = Gizzi coral. */
  accent?: string
  avatar: PetAvatar
  /** How the API runs this bot: `{ providerID: agent.provider, modelID: agent.model }`. */
  model?: ModelRef
}

/** Desktop's packaged assistant (allternit-ai useAgentBootstrap GIZZI_SEED). */
export const GIZZI_BOT_ID = 'gizzi-packaged-assistant'
export const GIZZI_BOT: PetBot = {
  id: GIZZI_BOT_ID,
  name: 'Gizzi',
  description: 'Your default assistant',
  avatar: { kind: 'gizzi' },
}

const SHAPES = ['circle', 'rounded', 'square', 'hex', 'diamond'] as const
const EYES = ['round', 'wide', 'narrow', 'focused', 'curious'] as const
const SPECIES = ['cat', 'dog', 'rabbit', 'fox', 'owl', 'robot'] as const
const ACCESSORIES = ['none', 'glasses', 'bow', 'headset'] as const
const HEX = /^#[0-9a-f]{6}$/i
const IMAGE_URL = /^(https?:\/\/|data:image\/)/i

function oneOf<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return allowed.includes(value as T) ? (value as T) : fallback
}
function color(value: unknown, fallback: string): string {
  return typeof value === 'string' && HEX.test(value) ? value : fallback
}
function obj(value: unknown): Record<string, unknown> | undefined {
  if (typeof value === 'string') {
    try {
      value = JSON.parse(value)
    } catch {
      return undefined
    }
  }
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined
}
function str(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined
}

export function parsePetAvatar(stored: unknown): PetAvatar {
  const avatar = obj(stored)
  const data = obj(avatar?.data)
  if (avatar?.type === 'geometric' && data) {
    return {
      kind: 'geometric',
      shape: oneOf(data.shape, SHAPES, 'circle'),
      primary: color(data.primaryColor, '#4FA3A5'),
      secondary: color(data.secondaryColor, '#27494A'),
      eyes: oneOf(data.eyePreset, EYES, 'round'),
    }
  }
  if (avatar?.type === 'pet' && data) {
    return {
      kind: 'pet',
      species: oneOf(data.species, SPECIES, 'cat'),
      primary: color(data.primaryColor, '#D4B08C'),
      secondary: color(data.secondaryColor, '#8A6A4F'),
      accessory: oneOf(data.accessory, ACCESSORIES, 'none'),
    }
  }
  const url = str(data?.url)
  if (avatar?.type === 'image' && url && IMAGE_URL.test(url)) return { kind: 'image', url }
  return { kind: 'gizzi' }
}

/** A legacy agent config avatar (`config.avatar`, `{ type: 'image', uri }`). */
function legacyImageAvatar(stored: unknown): PetAvatar | undefined {
  const avatar = obj(stored)
  const uri = str(avatar?.uri)
  return avatar?.type === 'image' && uri && IMAGE_URL.test(uri) ? { kind: 'image', url: uri } : undefined
}

function botAvatar(profileAvatar: unknown, configAvatar: unknown): PetAvatar {
  const avatar = parsePetAvatar(profileAvatar)
  return avatar.kind === 'gizzi' ? (legacyImageAvatar(configAvatar) ?? avatar) : avatar
}

/** One `/api/v1/agents` row → a pet bot, or undefined for non-bot agents. */
export function parsePetBot(raw: unknown): PetBot | undefined {
  const agent = obj(raw)
  const id = str(agent?.id)
  if (!agent || !id) return undefined
  const config = obj(agent.config)
  const isBot = agent.isBot ?? agent.is_bot ?? config?.isBot
  if (isBot !== true && isBot !== 1 && id !== GIZZI_BOT_ID) return undefined
  const profile = obj(agent.botProfile) ?? obj(agent.bot_profile) ?? obj(config?.botProfile)
  const provider = str(agent.provider)
  const model = str(agent.model)
  const bot: PetBot = {
    id,
    name: str(profile?.displayName) ?? str(agent.name) ?? 'Bot',
    description: str(profile?.tagline) ?? str(agent.description) ?? '',
    accent: typeof profile?.accentColor === 'string' && HEX.test(profile.accentColor) ? profile.accentColor : undefined,
    avatar: id === GIZZI_BOT_ID ? { kind: 'gizzi' } : botAvatar(profile?.avatar, config?.avatar),
    ...(provider && model ? { model: { providerID: provider, modelID: model } } : {}),
  }
  return bot
}

// ─── Store ──────────────────────────────────────────────────────────────────

/**
 * - connecting: first roster fetch in flight
 * - online: signed in and the API answered
 * - signed-out: no credential, or the API rejected it (run `gizzi login`)
 * - offline: the API couldn't be reached
 */
export type PetConnection = 'connecting' | 'online' | 'signed-out' | 'offline'

export type PetBotsState = {
  bots: PetBot[]
  /** The worn bot's id (shared with the Desktop pet). */
  currentId: string
  connection: PetConnection
  /** Set when a credential exists but the API rejected it. */
  expired?: boolean
  loading: boolean
}

let state: PetBotsState = { bots: [GIZZI_BOT], currentId: GIZZI_BOT_ID, connection: 'connecting', loading: false }
let initialized = false
const listeners = new Set<() => void>()

function set(patch: Partial<PetBotsState>) {
  state = { ...state, ...patch }
  for (const l of listeners) l()
}

function init() {
  if (initialized) return
  initialized = true
  const desktop = readDesktopPetAgentId()
  if (desktop) state = { ...state, currentId: desktop }
  watchDesktopPetAgentId(id => {
    set({ currentId: id ?? GIZZI_BOT_ID })
    // A bot picked in Desktop that we haven't listed yet: refresh the roster.
    if (id && !state.bots.some(b => b.id === id)) void refreshPetBots()
  })
}

export function getPetBotsState(): PetBotsState {
  init()
  return state
}

/** The bot the pet wears now. Unknown ids (not fetched yet, or offline) show Gizzi. */
export function getCurrentPetBot(): PetBot {
  const s = getPetBotsState()
  return s.bots.find(b => b.id === s.currentId) ?? GIZZI_BOT
}

export function subscribePetBots(listener: () => void): () => void {
  init()
  listeners.add(listener)
  return () => listeners.delete(listener)
}

export function usePetBots(): PetBotsState {
  return useSyncExternalStore(subscribePetBots, getPetBotsState)
}

export function useCurrentPetBot(): PetBot {
  const s = usePetBots()
  return s.bots.find(b => b.id === s.currentId) ?? GIZZI_BOT
}

let inFlight: Promise<void> | undefined
export function refreshPetBots(): Promise<void> {
  init()
  inFlight ??= (async () => {
    set({ loading: true })
    try {
      const { agents } = await platformRequest<{ agents: unknown[] }>('GET', '/api/v1/agents?is_bot=true', undefined, {
        timeoutMs: 10_000,
      })
      const fetched = (agents ?? []).map(parsePetBot).filter((b): b is PetBot => b !== undefined)
      const gizzi = fetched.find(b => b.id === GIZZI_BOT_ID) ?? GIZZI_BOT
      const others = fetched.filter(b => b.id !== GIZZI_BOT_ID).sort((a, b) => a.name.localeCompare(b.name))
      set({ bots: [gizzi, ...others], connection: 'online', expired: false, loading: false })
    } catch (err) {
      const rejected = err instanceof PlatformApiError && (err.status === 401 || err.status === 403)
      set({
        connection: err instanceof PlatformSignedOutError || rejected ? 'signed-out' : 'offline',
        expired: rejected,
        loading: false,
      })
    } finally {
      inFlight = undefined
    }
  })()
  return inFlight
}

/** Wear a bot here and on the Desktop pet. */
export function selectPetBot(id: string): void {
  init()
  set({ currentId: id })
  writeDesktopPetAgentId(id)
}

/** Test hook: reset module state. */
export function resetPetBotsForTests(): void {
  state = { bots: [GIZZI_BOT], currentId: GIZZI_BOT_ID, connection: 'connecting', loading: false }
  initialized = true
  listeners.clear()
}
