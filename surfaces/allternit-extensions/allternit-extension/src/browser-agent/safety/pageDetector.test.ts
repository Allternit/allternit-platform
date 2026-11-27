// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'

import { detectPageBlocker } from './pageDetector'

function page(html: string, title = ''): Document {
  const doc = document.implementation.createHTMLDocument(title)
  doc.body.innerHTML = html
  return doc
}

const filler = '<p>' + 'Lorem ipsum dolor sit amet. '.repeat(120) + '</p>'

describe('detectPageBlocker', () => {
  it('detects a visible password field as login', () => {
    const doc = page('<form><input name="email" type="email"><input type="password" name="pw"></form>')
    expect(detectPageBlocker(doc)?.kind).toBe('login')
  })

  it('ignores a hidden password field', () => {
    const doc = page('<div style="display:none"><input type="password"></div><h1>Dashboard</h1>')
    expect(detectPageBlocker(doc)).toBeNull()
  })

  it('detects a two-factor code prompt', () => {
    const doc = page(
      '<h1>Two-factor authentication</h1><form><input autocomplete="one-time-code" inputmode="numeric" maxlength="6"></form>',
    )
    expect(detectPageBlocker(doc)?.kind).toBe('two_factor')
  })

  it('detects a reCAPTCHA frame', () => {
    const doc = page('<iframe src="https://www.google.com/recaptcha/api2/bframe" width="400" height="580"></iframe>')
    expect(detectPageBlocker(doc)?.kind).toBe('captcha')
  })

  it('detects an hCaptcha widget', () => {
    const doc = page('<div class="h-captcha" data-sitekey="x"></div>')
    expect(detectPageBlocker(doc)?.kind).toBe('captcha')
  })

  it('detects a Turnstile frame', () => {
    const doc = page('<iframe src="https://challenges.cloudflare.com/cdn-cgi/challenge-platform/turnstile"></iframe>')
    expect(detectPageBlocker(doc)?.kind).toBe('captcha')
  })

  it('detects an access denied page', () => {
    const doc = page('<h1>Access Denied</h1><p>You do not have permission to view this page.</p>', 'Access Denied')
    expect(detectPageBlocker(doc)?.kind).toBe('block_page')
  })

  it('detects a rate-limit page', () => {
    const doc = page('<h1>429 Too Many Requests</h1><p>Please try again later.</p>', '429')
    expect(detectPageBlocker(doc)?.kind).toBe('rate_limited')
  })

  it('does not match a normal page, even one that mentions the words', () => {
    const doc = page(
      '<h1>Pricing</h1>' + filler + '<p>Access denied errors and rate limits are covered in our docs.</p>' +
        '<div class="grecaptcha-badge"><iframe src="https://www.google.com/recaptcha/api2/anchor"></iframe></div>' +
        '<form><input type="email" name="newsletter"><button>Subscribe</button></form>',
      'Pricing',
    )
    expect(detectPageBlocker(doc)).toBeNull()
  })
})
