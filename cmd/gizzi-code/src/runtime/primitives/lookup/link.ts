/**
 * lookup kind `link`: a preview of one HTTPS page (title, site name,
 * description, image, published date) from its Open Graph / meta tags.
 * Fetched through http.ts, so the webfetch SSRF guard refuses private and
 * loopback addresses on every redirect hop. Only https URLs; the image URL
 * must be https too. Credit: "Preview from <host>". Cache: TTL.link.
 *
 * Lane E1-2c fills this in.
 */
import { LookupError, type LinkParams, type LookupContext, type LookupResult } from "./types"

export async function lookupLink(params: LinkParams, ctx: LookupContext): Promise<LookupResult> {
  void params
  void ctx
  throw new LookupError("link", "Link preview is not available yet. Use webfetch.")
}
