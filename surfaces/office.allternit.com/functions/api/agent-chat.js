/**
 * Cloudflare Pages Function: /api/agent-chat
 *
 * Proxies the office apps' agent-chat requests to any OpenAI-compatible chat
 * completions endpoint. The API key stays server-side; the browser only sends
 * the instruction and an optional system prompt.
 *
 * Required environment variables (set in the Cloudflare Pages dashboard):
 *   MODEL_API_KEY  - Bearer token for the model API
 *   MODEL_API_BASE - Base URL for the provider, e.g. https://api.provider.com/v1
 *   MODEL_ID       - Model id, e.g. gpt-4o, claude-3-5-sonnet, etc.
 *
 * Every request must carry a valid Allternit Clerk session token
 * (`Authorization: Bearer <jwt>`), verified here against Clerk's JWKS, and come
 * from an Allternit origin. The model is always MODEL_ID; the browser cannot
 * pick another one on this key.
 */

// Clerk issuers Allternit uses (cmd/allternit-api/src/auth.rs).
const CLERK_ISSUERS = ['https://clerk.allternit.com', 'https://allternit.com/__clerk']
const ALLOWED_ORIGIN = /^https:\/\/(office\.allternit\.com|([a-z0-9-]+\.)?allternit-office\.pages\.dev)$|^http:\/\/localhost:\d+$/

function corsHeaders(origin) {
  const headers = {
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
    'Access-Control-Allow-Headers': 'Content-Type, Authorization',
    Vary: 'Origin',
  }
  if (origin && ALLOWED_ORIGIN.test(origin)) headers['Access-Control-Allow-Origin'] = origin
  return headers
}

function jsonError(status, error, cors) {
  return new Response(JSON.stringify({ error }), {
    status,
    headers: { ...cors, 'Content-Type': 'application/json' },
  })
}

function b64urlToBytes(value) {
  const b64 = value.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(value.length / 4) * 4, '=')
  return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))
}

function decodeJson(part) {
  return JSON.parse(new TextDecoder().decode(b64urlToBytes(part)))
}

const jwksCache = new Map() // issuer -> { keys, fetchedAt }

async function jwksFor(issuer) {
  const cached = jwksCache.get(issuer)
  if (cached && Date.now() - cached.fetchedAt < 10 * 60 * 1000) return cached.keys
  const res = await fetch(`${issuer}/.well-known/jwks.json`, { cf: { cacheTtl: 600 } })
  if (!res.ok) throw new Error(`jwks ${res.status}`)
  const { keys } = await res.json()
  jwksCache.set(issuer, { keys, fetchedAt: Date.now() })
  return keys
}

/** Verifies a Clerk session JWT (RS256). Returns its claims, or null. */
async function verifyClerkToken(token) {
  const parts = token.split('.')
  if (parts.length !== 3) return null
  let header, claims
  try {
    header = decodeJson(parts[0])
    claims = decodeJson(parts[1])
  } catch {
    return null
  }
  if (header.alg !== 'RS256' || !CLERK_ISSUERS.includes(claims.iss)) return null
  const now = Math.floor(Date.now() / 1000)
  if (typeof claims.exp !== 'number' || claims.exp < now - 5) return null
  if (typeof claims.nbf === 'number' && claims.nbf > now + 5) return null
  if (typeof claims.sub !== 'string' || !claims.sub) return null
  const jwk = (await jwksFor(claims.iss)).find((k) => k.kid === header.kid)
  if (!jwk) return null
  const key = await crypto.subtle.importKey('jwk', jwk, { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, false, ['verify'])
  const ok = await crypto.subtle.verify(
    'RSASSA-PKCS1-v1_5',
    key,
    b64urlToBytes(parts[2]),
    new TextEncoder().encode(`${parts[0]}.${parts[1]}`),
  )
  return ok ? claims : null
}

export async function onRequestOptions(context) {
  const origin = context.request.headers.get('origin')
  return new Response(null, {
    status: 204,
    headers: corsHeaders(origin),
  })
}

export async function onRequestPost(context) {
  const origin = context.request.headers.get('origin')
  const cors = corsHeaders(origin)

  if (origin && !ALLOWED_ORIGIN.test(origin)) return jsonError(403, 'origin not allowed', cors)

  const auth = context.request.headers.get('authorization') ?? ''
  const token = auth.startsWith('Bearer ') ? auth.slice(7).trim() : ''
  let claims = null
  try {
    claims = token ? await verifyClerkToken(token) : null
  } catch {
    return jsonError(503, 'could not check sign-in, try again', cors)
  }
  if (!claims) return jsonError(401, 'sign in to Allternit to use the AI assistant', cors)

  const env = context.env
  const apiKey = env.MODEL_API_KEY
  const apiBase = typeof env.MODEL_API_BASE === 'string' ? env.MODEL_API_BASE.trim().replace(/\/$/, '') : ''
  const model = typeof env.MODEL_ID === 'string' ? env.MODEL_ID.trim() : ''

  if (!apiKey || !apiBase || !model) {
    return new Response(
      JSON.stringify({
        error:
          'Model not configured. Set MODEL_API_KEY, MODEL_API_BASE, and MODEL_ID in the Pages environment variables.',
      }),
      {
        status: 500,
        headers: { ...cors, 'Content-Type': 'application/json' },
      },
    )
  }

  let body
  try {
    body = await context.request.json()
  } catch {
    return new Response(JSON.stringify({ error: 'invalid JSON body' }), {
      status: 400,
      headers: { ...cors, 'Content-Type': 'application/json' },
    })
  }

  const { message, systemPrompt } = body
  if (typeof message !== 'string' || !message.trim()) {
    return new Response(JSON.stringify({ error: 'missing message' }), {
      status: 400,
      headers: { ...cors, 'Content-Type': 'application/json' },
    })
  }

  const messages = []
  if (typeof systemPrompt === 'string' && systemPrompt.trim()) {
    messages.push({ role: 'system', content: systemPrompt.trim() })
  }
  messages.push({ role: 'user', content: message.trim() })

  const response = await fetch(`${apiBase}/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({
      model,
      messages,
      stream: true,
      stream_options: { include_usage: false },
    }),
  })

  if (!response.ok) {
    const text = await response.text().catch(() => 'unknown error')
    return new Response(JSON.stringify({ error: `upstream error ${response.status}`, detail: text }), {
      status: 502,
      headers: { ...cors, 'Content-Type': 'application/json' },
    })
  }

  const { readable, writable } = new TransformStream()
  const writer = writable.getWriter()
  const encoder = new TextEncoder()

  const streamUpstream = async () => {
    try {
      const reader = response.body.getReader()
      const decoder = new TextDecoder()
      let buffer = ''
      for (;;) {
        const { done, value } = await reader.read()
        if (done) break
        buffer += decoder.decode(value, { stream: true })
        const lines = buffer.split('\n')
        buffer = lines.pop() ?? ''
        for (const line of lines) {
          const trimmed = line.trim()
          if (!trimmed || !trimmed.startsWith('data: ')) continue
          const data = trimmed.slice(6)
          if (data === '[DONE]') continue
          let parsed
          try {
            parsed = JSON.parse(data)
          } catch {
            continue
          }
          const delta = parsed?.choices?.[0]?.delta
          const content = typeof delta?.content === 'string' ? delta.content : ''
          if (content) {
            const chunk = JSON.stringify({ chunk_type: 'text', chunk: content })
            await writer.write(encoder.encode(`data: ${chunk}\n\n`))
          }
        }
      }
      await writer.write(encoder.encode(`data: ${JSON.stringify({ chunk_type: 'done' })}\n\n`))
    } catch (err) {
      const errorChunk = JSON.stringify({ chunk_type: 'error', chunk: err instanceof Error ? err.message : String(err) })
      await writer.write(encoder.encode(`data: ${errorChunk}\n\n`))
    } finally {
      await writer.close()
    }
  }

  context.waitUntil(streamUpstream())

  return new Response(readable, {
    status: 200,
    headers: {
      ...cors,
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache',
      Connection: 'keep-alive',
    },
  })
}
