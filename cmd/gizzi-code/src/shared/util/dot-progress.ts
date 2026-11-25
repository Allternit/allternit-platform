/**
 * Braille dot progress for media and audio generation: the pure frame, its text and
 * the animate/still rules. Shared by `gizzi run` (cli/commands/run-print.ts) and the
 * Ink app (DotProgressLine).
 */

const DOT_REST = "\u2802" // one dot
const DOT_FILLED = "\u28FF" // all eight dots
// Light to dense, for the soft edges of the sweeping band.
const DOT_RAMP = ["\u2802", "\u2806", "\u2816", "\u2836", "\u28B6", "\u28F6", "\u28FE"]
const BAND_HALF = 3
export const DOT_FPS = 10

/**
 * One braille line: a light band sweeping across resting dots. Pure: the same
 * (width, t, fraction) always gives the same string.
 *
 * `t` is seconds. With a real `fraction` (0 to 1) the cells it has covered are
 * solid and the band sweeps only over the rest; without one the band sweeps the
 * whole width. A missing or non-finite `t` draws the still frame (no band).
 */
export function dotBandFrame(width: number, t: number | undefined, fraction?: number): string {
  const w = Math.max(1, Math.floor(width))
  const done = fraction === undefined || !Number.isFinite(fraction) ? 0 : Math.round(Math.min(1, Math.max(0, fraction)) * w)
  const span = w - done
  let out = DOT_FILLED.repeat(done)
  if (span <= 0) return out
  const moving = t !== undefined && Number.isFinite(t)
  // The band enters left of the open stretch and leaves past its right end.
  const cycle = span + BAND_HALF * 2
  const center = moving ? ((((t as number) * 14) % cycle) + cycle) % cycle - BAND_HALF : -Infinity
  for (let i = 0; i < span; i++) {
    const d = Math.abs(i - center)
    const level = Math.max(0, Math.round((1 - d / (BAND_HALF + 1)) * (DOT_RAMP.length - 1)))
    out += d <= BAND_HALF ? DOT_RAMP[level]! : DOT_REST
  }
  return out
}

export type ProgressDetailLite = { label?: string; fraction?: number; stage?: string }

export function progressDetailOf(meta: Record<string, unknown> | undefined): ProgressDetailLite | undefined {
  const d = meta?.progress_detail
  if (d && typeof d === "object") return d as ProgressDetailLite
  if (typeof meta?.progress === "string") return { label: meta.progress }
  return undefined
}

/** The text beside the dots: the real stage label, seconds, and a percent only when the fraction is real. */
export function dotProgressText(detail: ProgressDetailLite, elapsedS?: number): string {
  const bits = [detail.label || detail.stage || "Working"]
  if (elapsedS !== undefined) bits.push(`${Math.max(0, Math.floor(elapsedS))}s`)
  if (typeof detail.fraction === "number" && Number.isFinite(detail.fraction)) bits.push(`${Math.round(Math.min(1, Math.max(0, detail.fraction)) * 100)}%`)
  return bits.join(" \u00B7 ")
}

/** Animate only on a real, colored terminal, and not when the user asked for less motion. */
/** The Ink app's choice: animate, or draw one still frame. */
export function dotMode(reducedMotion: boolean, env: Record<string, string | undefined> = process.env, isTTY: boolean = !!process.stdout.isTTY): "animate" | "still" {
  return !reducedMotion && shouldAnimateDots(env, isTTY) ? "animate" : "still"
}

export function shouldAnimateDots(env: Record<string, string | undefined> = process.env, isTTY: boolean = !!process.stdout.isTTY): boolean {
  if (!isTTY) return false
  if (env.NO_COLOR !== undefined && env.NO_COLOR !== "") return false
  if (env.TERM === "dumb") return false
  const reduced = env.GIZZI_REDUCED_MOTION ?? env.REDUCED_MOTION
  if (reduced && reduced !== "0" && reduced.toLowerCase() !== "false") return false
  return true
}

