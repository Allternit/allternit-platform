/**
 * lookup kind `images`: LIMITS.imagesMin..imagesMax images from Wikimedia
 * Commons only (Openverse needs registration). Every image carries its author
 * credit, licence and the Commons file page; a result without a licence is
 * skipped. Commons has no safe-search, so results whose title or categories
 * hit an explicit-content word list are dropped here. Cache: TTL.images.
 */
import { fetchJson } from "./http"
import { LIMITS, LookupError, TTL, type ImagesParams, type LookupContext, type LookupResult } from "./types"

const API = "https://commons.wikimedia.org/w/api.php"
const CONCURRENCY = 3
const IMAGE_MIME = /^image\/(jpeg|png|webp|gif)$/

/** Words that make a Commons file unsuitable. Whole-word, case-insensitive. */
export const EXPLICIT_WORDS = [
  "nude", "nudity", "naked", "nsfw", "porn", "pornography", "pornographic", "erotic", "erotica",
  "sex", "sexual", "sexually", "genital", "genitals", "penis", "vagina", "vulva", "breast", "breasts",
  "topless", "fetish", "bdsm", "masturbation", "orgasm", "ejaculation", "hentai", "explicit", "gore",
]
const EXPLICIT = new RegExp(`\\b(${EXPLICIT_WORDS.join("|")})\\b`, "i")

interface ImageInfo {
  url?: string
  thumburl?: string
  descriptionurl?: string
  mime?: string
  extmetadata?: Record<string, { value?: string } | undefined>
}
interface Page {
  title?: string
  categories?: Array<{ title?: string }>
  imageinfo?: ImageInfo[]
}
interface CommonsResponse {
  query?: { pages?: Record<string, Page> | Page[] }
}

function plain(html: string | undefined): string {
  return (html ?? "")
    .replace(/<[^>]*>/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#0?39;/g, "'")
    .replace(/\s+/g, " ")
    .trim()
}

export interface CommonsImage {
  url: string
  alt: string
  caption: string
  credit: string
  license: string
  sourceUrl: string
}

/** Turn Commons pages into credited images; drops unlicensed, unsafe or non-image files. */
export function toImages(response: CommonsResponse): CommonsImage[] {
  const raw = response.query?.pages
  const pages = Array.isArray(raw) ? raw : Object.values(raw ?? {})
  const out: CommonsImage[] = []
  for (const page of pages) {
    const info = page.imageinfo?.[0]
    if (!info || !page.title) continue
    if (info.mime && !IMAGE_MIME.test(info.mime)) continue
    const ext = info.extmetadata ?? {}
    const license = plain(ext.LicenseShortName?.value)
    const author = plain(ext.Artist?.value) || plain(ext.Credit?.value)
    const url = info.thumburl ?? info.url
    if (!license || !author || !url || !info.descriptionurl) continue
    if (!url.startsWith("https://")) continue
    const name = page.title.replace(/^File:/, "").replace(/\.[a-z0-9]+$/i, "").replace(/_/g, " ")
    const cats = (page.categories ?? []).map((c) => c.title ?? "").join(" ")
    if (EXPLICIT.test(`${name} ${cats} ${plain(ext.ImageDescription?.value)}`)) continue
    const description = plain(ext.ImageDescription?.value)
    const licenseUrl = plain(ext.LicenseUrl?.value)
    out.push({
      url,
      alt: (description || name).slice(0, 200),
      caption: name,
      credit: `${author} / Wikimedia Commons`,
      license: licenseUrl ? `${license} (${licenseUrl})` : license,
      sourceUrl: info.descriptionurl,
    })
  }
  return out
}

async function pool<T, R>(items: T[], size: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  const results: R[] = new Array(items.length)
  let next = 0
  await Promise.all(
    Array.from({ length: Math.min(size, items.length) }, async () => {
      while (next < items.length) {
        const i = next++
        results[i] = await fn(items[i])
      }
    }),
  )
  return results
}

export async function lookupImages(params: ImagesParams, ctx: LookupContext): Promise<LookupResult> {
  const query = (params.query ?? "").trim()
  if (!query) throw new LookupError("images", "Say what to find images of.")
  if (EXPLICIT.test(query)) throw new LookupError("images", "That search is not something this lookup will fetch images for.")
  const want = Math.min(LIMITS.imagesMax, Math.max(LIMITS.imagesMin, Math.floor(params.count ?? LIMITS.imagesMin)))

  // One generator query returns search hits with imageinfo and categories together.
  const qs = new URLSearchParams({
    action: "query",
    format: "json",
    formatversion: "2",
    generator: "search",
    gsrsearch: `${query} filetype:bitmap`,
    gsrnamespace: "6",
    gsrlimit: String(want * 4),
    prop: "imageinfo|categories",
    iiprop: "url|mime|extmetadata",
    iiurlwidth: "800",
    iiextmetadatafilter: "LicenseShortName|LicenseUrl|Artist|Credit|ImageDescription",
    cllimit: "max",
  })
  let response: CommonsResponse
  try {
    // A single request; `pool` bounds concurrency for any follow-up pages.
    const [first] = await pool([`${API}?${qs}`], CONCURRENCY, (u) =>
      fetchJson<CommonsResponse>(u, { signal: ctx.signal, ttlMs: TTL.images }),
    )
    response = first
  } catch {
    throw new LookupError("images", "Wikimedia Commons did not answer. Try again, or use websearch.")
  }
  const images = toImages(response).slice(0, want)
  if (images.length === 0) throw new LookupError("images", `No licensed images found for "${query}".`)

  const credit = "Images: Wikimedia Commons, each under its own licence"
  const text = [
    `${images.length} image${images.length === 1 ? "" : "s"} for "${query}" (${credit}):`,
    ...images.map((i, n) => `${n + 1}. ${i.caption} - ${i.credit}, ${i.license}. ${i.sourceUrl}`),
  ].join("\n")
  return {
    text,
    card: {
      version: 1,
      template: "image-gallery",
      title: `Images: ${query}`,
      data: { images: images.map(({ url, alt, caption, credit: c, license, sourceUrl }) => ({ url, alt, caption, credit: c, license, sourceUrl })) },
    },
    credit,
  }
}
