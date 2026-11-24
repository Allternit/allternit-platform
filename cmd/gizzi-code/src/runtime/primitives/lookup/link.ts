/**
 * lookup kind `link`: a preview of one web page (title, site name,
 * description, image, published date) from its Open Graph / meta tags.
 * Fetched through http.ts (the webfetch SSRF guard, re-checked per redirect
 * hop; no cookies are sent). Literal private/loopback hosts are refused up
 * front with a clear message. The image URL must be https. Cache: TTL.link.
 */
import { isIP } from "node:net"
import { isEgressHostAllowed } from "@/shared/utils/hooks/ssrfGuard"
import { fetchText, LookupHttpError } from "./http"
import { LookupError, TTL, type LinkParams, type LookupContext, type LookupResult } from "./types"

const MAX_PAGE_BYTES = 512 * 1024
const HEAD_LIMIT = 200 * 1024

const ENTITIES: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " }
function decode(s: string): string {
  return s
    .replace(/&#x([0-9a-f]+);/gi, (_, h) => String.fromCodePoint(parseInt(h, 16)))
    .replace(/&#(\d+);/g, (_, d) => String.fromCodePoint(Number(d)))
    .replace(/&([a-z]+);/gi, (m, n) => ENTITIES[n.toLowerCase()] ?? m)
    .replace(/\s+/g, " ")
    .trim()
}

function attrs(tag: string): Record<string, string> {
  const out: Record<string, string> = {}
  for (const m of tag.matchAll(/([a-zA-Z:_-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/g)) {
    out[m[1].toLowerCase()] = m[2] ?? m[3] ?? m[4] ?? ""
  }
  return out
}

export interface PageMeta {
  title?: string
  siteName?: string
  description?: string
  image?: string
  imageAlt?: string
  published?: string
}

/** Read og:/twitter:/standard meta tags from the start of an HTML document. */
export function parseMeta(html: string, baseUrl: string): PageMeta {
  const head = html.slice(0, HEAD_LIMIT)
  const meta = new Map<string, string>()
  for (const m of head.matchAll(/<meta\b[^>]*>/gi)) {
    const a = attrs(m[0])
    const key = (a.property ?? a.name ?? "").toLowerCase()
    if (key && a.content !== undefined && !meta.has(key)) meta.set(key, decode(a.content))
  }
  const titleTag = /<title[^>]*>([\s\S]*?)<\/title>/i.exec(head)?.[1]
  const pick = (...keys: string[]) => keys.map((k) => meta.get(k)).find((v) => v) || undefined
  let image = pick("og:image:secure_url", "og:image", "twitter:image")
  if (image) {
    try {
      const u = new URL(image, baseUrl)
      image = u.protocol === "https:" ? u.href : undefined
    } catch {
      image = undefined
    }
  }
  return {
    title: pick("og:title", "twitter:title") ?? (titleTag ? decode(titleTag) || undefined : undefined),
    siteName: pick("og:site_name"),
    description: pick("og:description", "twitter:description", "description"),
    image,
    imageAlt: image ? pick("og:image:alt", "twitter:image:alt") : undefined,
    published: pick("article:published_time", "og:article:published_time", "date"),
  }
}

async function assertAllowed(url: URL): Promise<void> {
  const host = url.hostname.replace(/^\[|\]$/g, "").replace(/\.$/, "").toLowerCase()
  const literal = isIP(host) !== 0 || host === "localhost" || host.endsWith(".localhost")
  if (literal && !(await isEgressHostAllowed(host, { allowLoopback: false }))) {
    throw new LookupError("link", "That address is private or local, so it cannot be previewed.")
  }
}

export async function lookupLink(params: LinkParams, ctx: LookupContext): Promise<LookupResult> {
  let url: URL
  try {
    url = new URL((params.url ?? "").trim())
  } catch {
    throw new LookupError("link", "That is not a valid URL. Send a full http or https address.")
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new LookupError("link", "Only http and https links can be previewed.")
  }
  url.username = ""
  url.password = ""
  await assertAllowed(url)

  let page
  try {
    page = await fetchText(url.href, {
      signal: ctx.signal,
      ttlMs: TTL.link,
      maxBytes: MAX_PAGE_BYTES,
      headers: { Accept: "text/html,application/xhtml+xml" },
    })
  } catch (error) {
    if (error instanceof LookupError) throw error
    if (error instanceof LookupHttpError) throw new LookupError("link", `Could not read that page: ${error.message}`)
    const blocked = (error as { code?: string })?.code === "ERR_EGRESS_BLOCKED"
    throw new LookupError(
      "link",
      blocked ? "That address is private or local, so it cannot be previewed." : "Could not load that page. Try webfetch.",
    )
  }
  if (page.contentType && !/html/i.test(page.contentType)) {
    throw new LookupError("link", "That link is not a web page, so there is no preview. Try webfetch.")
  }
  const finalUrl = page.url || url.href
  const meta = parseMeta(page.text, finalUrl)
  const host = new URL(finalUrl).host
  const title = meta.title ?? host
  const credit = `Preview from ${host}`
  const lines = [title, meta.siteName ? `Site: ${meta.siteName}` : "", meta.description ?? "", meta.published ? `Published: ${meta.published}` : "", finalUrl, credit]
  return {
    text: lines.filter(Boolean).join("\n"),
    card: {
      version: 1,
      template: "link-preview",
      title,
      data: {
        url: finalUrl,
        title,
        siteName: meta.siteName,
        description: meta.description,
        image: meta.image,
        imageAlt: meta.imageAlt,
        published: meta.published,
        credit,
      },
    },
    credit,
  }
}
