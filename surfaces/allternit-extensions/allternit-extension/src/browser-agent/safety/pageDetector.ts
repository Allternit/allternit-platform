/**
 * Page detector for the agent's "needs you" pause.
 *
 * Pure DOM inspection (no chrome.* APIs) so it runs in the content script and
 * in unit tests. It never reads or returns field values.
 */

export type NeedsYouKind = 'login' | 'two_factor' | 'captcha' | 'block_page' | 'rate_limited'

export interface PageBlocker {
  kind: NeedsYouKind
  reason: string
}

const CAPTCHA_SRC = /recaptcha|hcaptcha|challenges\.cloudflare\.com|turnstile/i
const CAPTCHA_SELECTOR = '.g-recaptcha, .h-captcha, .cf-turnstile'

const RATE_LIMIT_TEXT =
  /too many requests|rate[ -]?limit(ed)?|error 429|\b429\b.{0,40}(too many|rate)|you are being rate|slow down|exceeded .{0,30}(limit|quota)/i
const BLOCK_TEXT =
  /access denied|attention required|just a moment|checking your browser|verify (that )?you are (a )?human|are you a robot|unusual traffic|you have been blocked|request blocked|security check|pardon our interruption|error 1020/i
const TWO_FACTOR_FIELD =
  /one-time-code|\b(otp|totp|2fa|mfa)\b|verification[-_ ]?code|security[-_ ]?code|authenticat(or|ion)[-_ ]?code|auth[-_ ]?code/i
const TWO_FACTOR_PAGE =
  /two[- ]factor|2[- ]step|two[- ]step|verification code|authenticator app|enter the (6|six)[- ]digit|security code/i
const SIGN_IN_BUTTON = /^\s*(sign|log)[ -]?in\s*$|^\s*continue\s*$|^\s*next\s*$/i

function isVisible(el: Element): boolean {
  const view = el.ownerDocument.defaultView
  let node: Element | null = el
  while (node) {
    if (node.hasAttribute('hidden')) return false
    if (node instanceof HTMLInputElement && node.type === 'hidden') return false
    const style = view?.getComputedStyle(node)
    if (style && (style.display === 'none' || style.visibility === 'hidden')) return false
    node = node.parentElement
  }
  if (el instanceof HTMLIFrameElement) {
    const w = el.getAttribute('width')
    const h = el.getAttribute('height')
    if (w === '0' || h === '0') return false
  }
  return true
}

function visibleInputs(doc: Document): HTMLInputElement[] {
  return Array.from(doc.querySelectorAll('input')).filter(isVisible)
}

/** Short pages (error/interstitial pages) are where block text is trustworthy. */
function shortPageText(doc: Document): string {
  const body = (doc.body?.innerText ?? doc.body?.textContent ?? '').replace(/\s+/g, ' ').trim()
  const title = doc.title || ''
  const h1 = doc.querySelector('h1')?.textContent ?? ''
  const head = `${title} ${h1}`
  return body.length < 2000 ? `${head} ${body}` : head
}

function hasCaptcha(doc: Document): boolean {
  for (const frame of Array.from(doc.querySelectorAll('iframe'))) {
    const src = frame.getAttribute('src') || ''
    if (!CAPTCHA_SRC.test(src)) continue
    // The invisible reCAPTCHA badge is not a challenge.
    if (frame.closest('.grecaptcha-badge')) continue
    if (isVisible(frame)) return true
  }
  for (const el of Array.from(doc.querySelectorAll(CAPTCHA_SELECTOR))) {
    if (el.getAttribute('data-size') === 'invisible' || el.tagName === 'BUTTON') continue
    if (isVisible(el)) return true
  }
  return false
}

function hasTwoFactor(doc: Document): boolean {
  const inputs = visibleInputs(doc).filter((i) => ['text', 'tel', 'number', 'password', ''].includes(i.type))
  for (const input of inputs) {
    const hint = [
      input.getAttribute('autocomplete'),
      input.name,
      input.id,
      input.placeholder,
      input.getAttribute('aria-label'),
    ]
      .filter(Boolean)
      .join(' ')
    if (TWO_FACTOR_FIELD.test(hint)) return true
  }
  const code = inputs.find((i) => {
    const max = Number(i.getAttribute('maxlength'))
    return max >= 4 && max <= 8 && (i.inputMode === 'numeric' || i.type === 'tel' || i.type === 'number')
  })
  return Boolean(code && TWO_FACTOR_PAGE.test(shortPageText(doc)))
}

function hasLogin(doc: Document): boolean {
  if (visibleInputs(doc).some((i) => i.type === 'password')) return true
  // First step of a two-step sign-in: username field plus a sign-in button.
  for (const form of Array.from(doc.querySelectorAll('form'))) {
    if (!isVisible(form)) continue
    const user = Array.from(form.querySelectorAll('input')).find(
      (i) =>
        isVisible(i) &&
        (/username/i.test(i.getAttribute('autocomplete') || '') ||
          /^(user(name)?|login|identifier)$/i.test(i.name || i.id || '')),
    )
    if (!user) continue
    const button = Array.from(form.querySelectorAll('button, input[type=submit]')).find((b) =>
      SIGN_IN_BUTTON.test((b as HTMLInputElement).value || b.textContent || ''),
    )
    if (button) return true
  }
  return false
}

export function detectPageBlocker(doc: Document): PageBlocker | null {
  if (hasCaptcha(doc)) return { kind: 'captcha', reason: 'A captcha is showing on this page.' }

  const text = shortPageText(doc)
  if (RATE_LIMIT_TEXT.test(text)) {
    return { kind: 'rate_limited', reason: 'The site says there were too many requests.' }
  }
  if (BLOCK_TEXT.test(text)) {
    return { kind: 'block_page', reason: 'The site is showing a block or verification page.' }
  }
  if (hasTwoFactor(doc)) {
    return { kind: 'two_factor', reason: 'The site is asking for a verification code.' }
  }
  if (hasLogin(doc)) return { kind: 'login', reason: 'The site is asking you to sign in.' }
  return null
}
