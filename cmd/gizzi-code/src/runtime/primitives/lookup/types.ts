/**
 * Types for the `lookup` primitive: each kind's parameters, the card data it
 * returns, and the result envelope `{ text, card?, credit }`.
 *
 * The card data mirrors the answer catalog templates (weather, time-zones,
 * conversion, link-preview, places, image-gallery) in
 * runtime/integrations/artifacts/answer-catalog.ts, which is synced from
 * allternit-ai. index.ts validates every card against that contract before it
 * leaves the runtime, so a field that drifts here is dropped, not shown.
 *
 * Free of runtime state and UI imports so the Ink adapter and bun tests can
 * use it.
 */

export const LOOKUP_KINDS = ["weather", "time", "convert", "link", "places", "images"] as const
export type LookupKind = (typeof LOOKUP_KINDS)[number]

/** Kinds that never touch the network (unit conversion is local too; see index.ts). */
export const LOCAL_KINDS: ReadonlySet<LookupKind> = new Set(["time"])

/** Limits (Wave E proposals). */
export const LIMITS = {
  imagesMin: 3,
  imagesMax: 5,
  placesMax: 10,
  forecastDays: 7,
  zonesMax: 12,
  conversionsAlsoMax: 8,
} as const

/** Cache lifetimes in ms (http.ts cache). */
export const TTL = {
  weather: 10 * 60_000,
  fx: 12 * 60 * 60_000,
  geocode: 24 * 60 * 60_000,
  link: 10 * 60_000,
  images: 60 * 60_000,
} as const

/** What the model sends (the `lookup` tool's parameters). */
export interface LookupParams {
  kind: LookupKind
  /** Free text: a place, a search, a URL (link) or an expression ("9am Chicago in Tokyo"). */
  query: string
  place?: string
  /** IANA zones or place names for `time`. */
  zones?: string[]
  amount?: number
  from?: string
  to?: string
  count?: number
}

/** The opt-in shared location (allternit-api client_location). Present only when the user turned it on. */
export interface ClientLocation {
  lat: number
  lon: number
  label?: string
}

/** Per-call context the dispatcher hands every kind. */
export interface LookupContext {
  signal: AbortSignal
  /** Only set when the user shares their location; "here" without it means: ask for a place. */
  location?: ClientLocation
  /** This turn's IANA zone (Clock.turnZoneOf), for "now" and local dates. */
  timeZone?: string
  /** Test seam for the clock. */
  now?: () => Date
}

// ── Per-kind parameters ──────────────────────────────────────────────────────

export interface WeatherParams {
  /** Place name to geocode; empty means the shared location. */
  place?: string
  /** Forecast days, 0..LIMITS.forecastDays. */
  days?: number
  unit?: "C" | "F"
}

export interface PlacesParams {
  query: string
  /** Where to search near; empty means the shared location. */
  near?: string
  count?: number
}

export interface ConvertParams {
  amount: number
  from: string
  to: string
}

export interface ZonesParams {
  /** Reference moment as written ("now", "9am Chicago"); empty means now. */
  query?: string
  /** IANA zones or place names. */
  zones: string[]
}

export interface LinkParams {
  url: string
}

export interface ImagesParams {
  query: string
  count?: number
}

// ── Card data (same shape as the catalog templates) ──────────────────────────

export interface WeatherData {
  location: string
  observedAt: string
  temperature: number
  unit: "C" | "F"
  conditions: string
  humidity: number
  wind: string
  source: string
  forecast?: Array<{ day: string; high: number; low: number; conditions: string; precipitation?: string }>
  credit?: string
  creditUrl?: string
}

export interface TimeZonesData {
  reference: string
  zones: Array<{ place: string; zone: string; time: string; date: string; offset: string }>
  credit: string
}

export interface ConversionData {
  kind: "unit" | "currency"
  from: { value: number; unit: string }
  to: { value: number; unit: string }
  rate?: number
  asOf?: string
  also?: Array<{ value: number; unit: string }>
  credit: string
  creditUrl?: string
}

export interface LinkPreviewData {
  url: string
  title: string
  siteName?: string
  description?: string
  image?: string
  imageAlt?: string
  published?: string
  credit: string
}

export interface PlacesData {
  query: string
  near?: string
  places: Array<{ name: string; address: string; category?: string; distance?: string; lat?: number; lon?: number; url?: string }>
  credit: string
  creditUrl?: string
}

export interface ImageGalleryData {
  images: Array<{ url: string; alt: string; caption: string; credit?: string; license?: string; sourceUrl?: string }>
}

export interface LookupCardData {
  weather: WeatherData
  "time-zones": TimeZonesData
  conversion: ConversionData
  "link-preview": LinkPreviewData
  places: PlacesData
  "image-gallery": ImageGalleryData
}
export type LookupCardTemplate = keyof LookupCardData

/** A catalog answer envelope; clients draw it from the tool part's metadata. */
export type LookupCard = {
  [T in LookupCardTemplate]: { version: 1; template: T; title: string; summary?: string; data: LookupCardData[T] }
}[LookupCardTemplate]

/** What every kind returns. `text` stands alone for clients that draw no card. */
export interface LookupResult {
  text: string
  card?: LookupCard
  /** Provider credit, always shown ("Weather data: MET Norway"). */
  credit: string
}

/** A kind could not answer; the message is safe to show and says what to try instead. */
export class LookupError extends Error {
  constructor(
    readonly kind: LookupKind,
    message: string,
  ) {
    super(message)
    this.name = "LookupError"
  }
}
