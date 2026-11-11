import {
  PET_OFFER_COPY,
  PET_OFFER_LIMITS,
  PET_OFFER_RULES,
  petOfferSettingsDefaults,
  type PetOfferKind,
} from './petOffers.gen'

/**
 * Pet offers for this terminal (contract: contracts/pet-offers/pet-offers-v1.json).
 * The pet only ever looks at this session's own output: a failed shell command,
 * an agent Bash result, a turn error. Nothing here logs or sends the text.
 */

export type PetOfferMatch = { kind: PetOfferKind; ruleId: string; line: string }

const compiled = PET_OFFER_RULES.map(rule => ({ rule, re: new RegExp(rule.pattern, rule.flags) }))

/** Contract matcher: first rule in file order wins; line = whole line at the match start. */
export function matchPetOffer(
  rawText: string,
  opts: { dialog?: boolean; maxTextChars?: number } = {},
): PetOfferMatch | undefined {
  const max = opts.maxTextChars ?? petOfferSettingsDefaults().maxTextChars
  let text = rawText.replace(/\r\n?/g, '\n')
  if (text.length > max) text = text.slice(-max)
  for (const { rule, re } of compiled) {
    if (rule.scope === 'dialog' && !opts.dialog) continue
    const m = re.exec(text)
    if (!m) continue
    const start = text.lastIndexOf('\n', m.index - 1) + 1
    const end = text.indexOf('\n', m.index)
    const line = text
      .slice(start, end === -1 ? undefined : end)
      .trim()
      .slice(0, PET_OFFER_LIMITS.lineMaxChars)
    return { kind: rule.kind, ruleId: rule.id, line }
  }
  return undefined
}

/** Bubble copy for a kind. The trailing hint names the one way to accept. */
export function petOfferBubble(kind: PetOfferKind): string {
  return `${PET_OFFER_COPY.byKind[kind].replace(PET_OFFER_COPY.appPlaceholder, PET_OFFER_COPY.appFallback)} Type /fix`
}

/**
 * One offer per working directory per cooldown, and never the same matched
 * line twice for one directory. `allow` records the offer when it says yes.
 */
export function createOfferGate() {
  const lastAt = new Map<string, number>()
  const seen = new Map<string, Set<string>>()
  return {
    allow(cwd: string, line: string, now: number, cooldownMs = petOfferSettingsDefaults().cooldownMs): boolean {
      const at = lastAt.get(cwd)
      if (at !== undefined && now - at < cooldownMs) return false
      const lines = seen.get(cwd) ?? new Set<string>()
      if (lines.has(line)) return false
      lines.add(line)
      seen.set(cwd, lines)
      lastAt.set(cwd, now)
      return true
    },
  }
}

export const petOfferGate = createOfferGate()

export type PetFailure = { cwd: string; kind: PetOfferKind; line: string; tail: string; at: number }

let lastFailure: PetFailure | undefined

/** What /fix hands to the model: the last failure, text cut to the contract's tailChars. */
export function recordPetFailure(f: { cwd: string; kind: PetOfferKind; line: string; text: string }): PetFailure {
  const tailChars = petOfferSettingsDefaults().tailChars
  lastFailure = { cwd: f.cwd, kind: f.kind, line: f.line, tail: f.text.replace(/\r\n?/g, '\n').slice(-tailChars), at: Date.now() }
  return lastFailure
}

export function getLastPetFailure(): PetFailure | undefined {
  return lastFailure
}

export function buildFixPrompt(f: PetFailure | undefined): string {
  const base = 'The last command failed. Find the cause and fix it.'
  if (!f) return base
  return `${base}\n\nMatched line: ${f.line}\n\nEnd of the output:\n\`\`\`\n${f.tail}\n\`\`\``
}

export function firstUsefulLine(text: string): string {
  const line = text.replace(/\r\n?/g, '\n').split('\n').map(l => l.trim()).filter(Boolean).at(-1) ?? ''
  return line.slice(0, PET_OFFER_LIMITS.lineMaxChars)
}

/**
 * Decide whether a failure becomes an offer. Always records it for /fix.
 * `needMatch`: agent Bash results only offer when a contract rule matches;
 * user shell failures and turn errors fall back to run_failed.
 */
export function evaluatePetFailure(args: {
  cwd: string
  text: string
  needMatch: boolean
  enabled: boolean
  now?: number
}): { offer: string | undefined; failure: PetFailure } | undefined {
  const m = matchPetOffer(args.text)
  if (!m && args.needMatch) return undefined
  const kind = m?.kind ?? 'run_failed'
  const line = m?.line ?? firstUsefulLine(args.text)
  const failure = recordPetFailure({ cwd: args.cwd, kind, line, text: args.text })
  const offer =
    args.enabled && petOfferGate.allow(args.cwd, line, args.now ?? Date.now()) ? petOfferBubble(kind) : undefined
  return { offer, failure }
}
