/**
 * Turns a provider failure into its real cause plus one or two commands to
 * run. Every model path in the TUI (Allternit Cloud, OpenAI-compatible API
 * keys and local servers, CLI brains) maps its error text here, so an
 * expired OpenRouter key no longer reads "Local model error" and a used-up
 * Cloud allowance no longer reads "Not logged in".
 *
 * Pure functions: no network, no config reads. Callers pass what they know.
 * Only subscriptionAlternative reads discovery, loaded on call, so this file
 * stays a leaf the API layer can import without pulling in the runtime.
 */

export type ProviderFailure = {
  /** Provider id ("allternit", "openrouter", "claude-cli", "local-mlx"…). */
  provider?: string
  /** Human name for the provider, when the caller has one. */
  providerName?: string
  /** HTTP status, when the failure came from an HTTP API. */
  status?: number
  /** The error body or message as received. */
  text: string
  /** The server's base URL, for connection failures. */
  baseURL?: string
  /** A CLI brain's binary (claude, codex, kimi), for sign-in hints. */
  cliBin?: string
  /** A model on the user's own subscription to suggest, e.g. "claude-cli/claude-sonnet-5". */
  alternative?: { model: string; label: string }
  now?: Date
}

export type ProviderErrorHelp = {
  /** The real cause, one line. */
  message: string
  /** Commands to run, each with what it does. */
  commands: Array<{ command: string; why: string }>
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

/** "Dec 1": Allternit Cloud monthly limits reset on the 1st (UTC). */
export function nextMonthStart(now = new Date()): string {
  const month = (now.getUTCMonth() + 1) % 12
  return `${MONTHS[month]} 1`
}

const KNOWN_NAMES: Record<string, string> = {
  allternit: 'Allternit Cloud',
  openrouter: 'OpenRouter',
  'claude-cli': 'Claude Code',
  'codex-cli': 'Codex',
  'kimi-cli': 'Kimi CLI',
}

function nameOf(f: ProviderFailure): string {
  if (f.providerName) return f.providerName
  if (f.provider && KNOWN_NAMES[f.provider]) return KNOWN_NAMES[f.provider]!
  return f.provider ?? 'The provider'
}

function switchHint(f: ProviderFailure): { command: string; why: string } {
  return f.alternative
    ? { command: `/model ${f.alternative.model}`, why: f.alternative.label }
    : { command: '/model', why: 'pick another model' }
}

function isCloud(f: ProviderFailure): boolean {
  return f.provider === 'allternit' || f.provider === undefined
}

/**
 * The real cause of a provider failure, or undefined when the text matches
 * no known cause (callers then show the provider's own message).
 */
export function describeProviderError(f: ProviderFailure): ProviderErrorHelp | undefined {
  const text = f.text ?? ''
  const lower = text.toLowerCase()
  const name = nameOf(f)
  const resets = nextMonthStart(f.now)

  // Allternit Cloud: the free monthly allowance (403 Forbidden).
  const allowance = text.match(/free inference allowance of \$?([\d.]+)\/month is used up/i)
  if (allowance) {
    return {
      message: `Allternit Cloud free allowance ($${allowance[1]}/month) is used up · resets ${resets}`,
      commands: [switchHint(f), { command: '/usage', why: 'see your plan and credits' }],
    }
  }
  // Allternit Cloud: plan or credits at their limit.
  if (/inference credits exhausted/i.test(text)) {
    return {
      message: 'Allternit Cloud credits are used up',
      commands: [switchHint(f), { command: '/usage', why: 'add credits or change plan' }],
    }
  }
  if (isCloud(f) && (f.status === 402 || /(free plan|plan).{0,40}(monthly )?limit|monthly limit/i.test(text))) {
    return {
      message: `Allternit Cloud plan limit reached for this month · resets ${resets}`,
      commands: [switchHint(f), { command: '/usage', why: 'see your plan and credits' }],
    }
  }
  if (/requires a verified email/i.test(text)) {
    return {
      message: 'Allternit Cloud free models need a verified email address',
      commands: [
        { command: '/login', why: 'sign in again after you verify the email' },
        switchHint(f),
      ],
    }
  }

  // A CLI brain that is not installed or not signed in.
  if (/no runtime found for cli/i.test(text)) {
    return {
      message: `${name} isn't installed on this computer`,
      commands: [
        { command: `install ${f.cliBin ?? name}, then gizzi agents harness register`, why: 'add it as a brain' },
        switchHint(f),
      ],
    }
  }
  if (f.cliBin && /not logged in|please run \/login|not signed in|login required|unauthenticated|invalid api key|authentication/i.test(text)) {
    return {
      message: `${name} isn't signed in on this computer`,
      commands: [
        {
          command: f.cliBin === 'claude' ? 'claude, then /login' : `${f.cliBin} login`,
          why: `sign in to ${name}`,
        },
        switchHint(f),
      ],
    }
  }

  // HTTP APIs (API keys, OpenAI-compatible servers, Cloud sign-in).
  // A model the provider or CLI doesn't know: its own words, then /model.
  if (/issue with the selected model|model .{0,60}(does not exist|doesn't exist|not found|not available|not supported)|unknown model|invalid model/i.test(text)) {
    return {
      message: `${name}: ${text.trim().split(/\r?\n/)[0]}`,
      commands: [{ command: '/model', why: 'pick a model it serves' }],
    }
  }
  if (f.status === 401 || (f.status === 403 && /key|token|auth|unauthori[sz]ed|forbidden/i.test(lower))) {
    if (isCloud(f)) {
      return {
        message: 'Allternit Cloud sign-in expired or missing',
        commands: [{ command: '/login', why: 'sign in again' }, switchHint(f)],
      }
    }
    return {
      message: `${name} rejected the API key (${f.status}) · it may be expired or revoked`,
      commands: [
        { command: `gizzi connect login`, why: `save a new ${name} key` },
        switchHint(f),
      ],
    }
  }
  if (f.status === 402 || /insufficient (credits|funds|balance)|payment required|quota exceeded/i.test(text)) {
    return {
      message: `${name} account is out of credits`,
      commands: [switchHint(f)],
    }
  }
  if (f.status === 429 || /rate limit|too many requests/i.test(lower)) {
    return {
      message: `${name} rate limit hit · wait a moment and retry`,
      commands: [switchHint(f)],
    }
  }
  if (f.status === 404 && /model/i.test(lower)) {
    return {
      message: `${name} doesn't serve this model`,
      commands: [{ command: '/model', why: 'pick one it serves' }],
    }
  }
  if (/econnrefused|connection refused|unable to connect|fetch failed/i.test(lower)) {
    return {
      message: f.baseURL ? `Nothing is answering at ${f.baseURL}` : `${name} is unreachable`,
      commands: [switchHint(f)],
    }
  }
  return undefined
}

/** One line for the transcript: "cause · run `cmd` (why) or `cmd` (why)". */
export function formatProviderErrorHelp(help: ProviderErrorHelp): string {
  if (help.commands.length === 0) return help.message
  const steps = help.commands.map(c => `\`${c.command}\` (${c.why})`).join(' or ')
  return `${help.message}\nRun ${steps}`
}

/**
 * A model on the user's own subscription to offer when a Cloud or API-key
 * model fails ("/model claude-cli/claude-sonnet-5 (your Claude Code plan)"):
 * the first installed CLI brain, its Sonnet when it has one.
 */
export function subscriptionAlternative(exclude?: string): { model: string; label: string } | undefined {
  try {
    const { Discovery } = require('../../../../../runtime/providers/discovery/index.js') as typeof import('../../../../../runtime/providers/discovery/index.js')
    const brains = Discovery.last().filter(p => p.source === 'subprocess' && p.id !== exclude && p.models.length > 0)
    const pick = brains.find(p => p.id === 'claude-cli') ?? brains[0]
    if (!pick) return undefined
    const model = pick.models.find(m => /sonnet/i.test(m.id)) ?? pick.models[0]!
    return { model: `${pick.id}/${model.id}`, label: `your ${pick.name} plan` }
  } catch {
    return undefined
  }
}
