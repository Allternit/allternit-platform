/**
 * Lookup answer cards as aligned terminal text. One pure function per
 * template (weather, time-zones, conversion, link-preview, places,
 * image-gallery), so the Ink result view and transcript search show the
 * same lines. The credit is always its own last line.
 */
import type { LookupCard } from '@/runtime/primitives/lookup'
import { stringWidth } from '../../ink/stringWidth'

export type CardText = { lines: string[]; credit?: string }

const GAP = '  '

function pad(text: string, width: number): string {
  return text + ' '.repeat(Math.max(0, width - stringWidth(text)))
}

/** Rows as columns padded to the widest cell; the last column is not padded. */
export function alignRows(rows: string[][]): string[] {
  const widths: number[] = []
  for (const row of rows) row.forEach((cell, i) => (widths[i] = Math.max(widths[i] ?? 0, stringWidth(cell))))
  return rows.map(row => row.map((cell, i) => (i === row.length - 1 ? cell : pad(cell, widths[i]!))).join(GAP).trimEnd())
}

function num(value: number): string {
  return Number.isInteger(value) ? String(value) : String(Number(value.toFixed(6)))
}

/** "2026-11-17T14:00:00Z" → "2026-11-17 14:00"; anything else as given. */
function when(value: string): string {
  const m = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})/.exec(value)
  return m ? `${m[1]} ${m[2]}` : value
}

function credited(credit: string | undefined, url?: string): string | undefined {
  if (!credit) return undefined
  return url && !credit.includes(url) ? `${credit} (${url})` : credit
}

export function cardText(card: LookupCard): CardText {
  switch (card.template) {
    case 'weather': {
      const d = card.data
      const lines = [
        `${d.temperature}°${d.unit}  ${d.conditions}`,
        `Humidity ${d.humidity}%  ·  Wind ${d.wind}  ·  Observed ${when(d.observedAt)}`,
      ]
      if (d.forecast?.length) {
        lines.push(
          '',
          ...alignRows([
            ['Day', 'High', 'Low', 'Conditions', 'Precip.'],
            ...d.forecast.map(f => [f.day, `${f.high}°`, `${f.low}°`, f.conditions, f.precipitation ?? '']),
          ]),
        )
      }
      return { lines, credit: credited(d.credit ?? d.source, d.creditUrl) }
    }
    case 'time-zones': {
      const d = card.data
      return {
        lines: [
          d.reference,
          '',
          ...alignRows([['Place', 'Time', 'Date', 'UTC'], ...d.zones.map(z => [z.place, z.time, z.date, z.offset])]),
        ],
        credit: credited(d.credit),
      }
    }
    case 'conversion': {
      const d = card.data
      const lines = [`${num(d.from.value)} ${d.from.unit} = ${num(d.to.value)} ${d.to.unit}`]
      const rate = d.rate !== undefined ? `Rate 1 ${d.from.unit} = ${num(d.rate)} ${d.to.unit}` : undefined
      if (rate || d.asOf) lines.push([rate, d.asOf ? `as of ${d.asOf}` : undefined].filter(Boolean).join(', '))
      if (d.also?.length) lines.push(`Also: ${d.also.map(a => `${num(a.value)} ${a.unit}`).join(' · ')}`)
      return { lines, credit: credited(d.credit, d.creditUrl) }
    }
    case 'link-preview': {
      const d = card.data
      const lines = [d.title]
      const meta = [d.siteName, d.published].filter(Boolean).join(' · ')
      if (meta) lines.push(meta)
      if (d.description) lines.push(d.description)
      lines.push(d.url)
      return { lines, credit: credited(d.credit) }
    }
    case 'places': {
      const d = card.data
      const lines = [d.near ? `${d.query} · near ${d.near}` : d.query, '']
      if (!d.places.length) lines.push('No places found.')
      d.places.forEach((p, i) => {
        const tag = [p.category, p.distance].filter(Boolean).join(' · ')
        lines.push(`${i + 1}. ${p.name}${tag ? `  (${tag})` : ''}`, `   ${p.address}`)
        if (p.url) lines.push(`   ${p.url}`)
      })
      return { lines, credit: credited(d.credit, d.creditUrl) }
    }
    case 'image-gallery': {
      const lines: string[] = []
      card.data.images.forEach((img, i) => {
        lines.push(`${i + 1}. ${img.caption || img.alt}`)
        const credit = [img.credit, img.license].filter(Boolean).join(', ')
        if (credit) lines.push(`   ${credit}`)
        lines.push(`   ${img.sourceUrl ?? img.url}`)
      })
      return { lines }
    }
  }
}

/** Every line the result view shows, for transcript search. */
export function cardSearchText(card: LookupCard): string {
  const { lines, credit } = cardText(card)
  return [card.title, card.summary, ...lines, credit].filter(Boolean).join('\n')
}
