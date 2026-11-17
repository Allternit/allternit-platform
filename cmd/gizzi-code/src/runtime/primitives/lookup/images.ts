/**
 * lookup kind `images`: LIMITS.imagesMin..imagesMax images from Wikimedia
 * Commons, then Openverse when Commons has too few. Every image carries its
 * author credit, licence and source page; an image without them is skipped.
 * Commons has no safe-search filter of its own (plan risk); this module owns any filtering.
 * Cache: TTL.images.
 *
 * Lane E1-2c fills this in.
 */
import { LookupError, type ImagesParams, type LookupContext, type LookupResult } from "./types"

export async function lookupImages(params: ImagesParams, ctx: LookupContext): Promise<LookupResult> {
  void params
  void ctx
  throw new LookupError("images", "Image search is not available yet.")
}
